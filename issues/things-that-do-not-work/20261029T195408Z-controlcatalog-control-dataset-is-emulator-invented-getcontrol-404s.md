Issue-Source: trace-sweep-divergence
Issue-Trace: controlcatalog-catalog-get-control
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: controlcatalog-catalog-get-control controlcatalog-catalog-list-controls-aws-config-configrule controlcatalog-catalog-list-controls-aws-securityhub-securitycontrol
Issue-Shape-Id: com.amazonaws.controlcatalog#ControlCatalog
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.Aliases
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.Arn
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.Behavior
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.CreateTime
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.Description
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.GovernedProviders
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.GovernedResources
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.Implementation
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.Name
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.ParameterRequirementSummary
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.Parameters
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.RegionConfiguration
Issue-Key: controlcatalog|GetControl|2xx->4xx|$.Severity
Issue-Key: controlcatalog|GetControl|2xx->4xx|status
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[*]
Issue-Commit: 128b971c27980fd8fd6bfdfd4c5ec95349a617e4

# ControlCatalog control dataset is emulator-invented: GetControl 404s and ListControls serves placeholder entries

> _Note: this issue was identified against the code base at commit `128b971c27980fd8fd6bfdfd4c5ec95349a617e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetControl

## Description
The emulator does not serve the AWS ControlCatalog control dataset. It neither resolves the catalog's control ARNs on GetControl nor returns the catalog's controls from ListControls; instead ListControls answers with a small set of emulator-authored control records whose ARNs, names, descriptions, timestamps and parameter summaries are not the catalog's. Two causes.

Cause 1 of 2 - GetControl does not resolve catalog control ARNs (fixture controlcatalog-catalog-get-control, step task#1).
AWS: Smithy GetControl (POST /get-control, http code 200) takes a required ControlArn matching ^arn:(aws(?:[-a-z]*)?):(controlcatalog|controltower):[a-zA-Z0-9-]*::control/[0-9a-zA-Z_\-]+$ and returns 200 with the required members Arn, Name, Description, Behavior and RegionConfiguration plus Aliases, Severity, Implementation, ParameterRequirementSummary, Parameters, CreateTime, GovernedResources and GovernedProviders. ResourceNotFoundException (404) is modeled only for a control ARN that is not in the catalog; every ARN the fixture sends is a catalog control (recorded response: 200 with the full control body).
Emulator: every GetControl in the fixture answers 404 and the whole body is absent - Aliases, Arn, Behavior, CreateTime, Description, GovernedProviders, GovernedResources, Implementation, Name, ParameterRequirementSummary, Parameters, RegionConfiguration and Severity are all missing. The cluster covers 20 steps (task#1 through task#20), i.e. every control looked up.
Why: a client that reads a control's Behavior/Severity/Implementation/RegionConfiguration before enabling or reporting on it sees ResourceNotFoundException for controls that exist, so catalog-driven governance code treats the entire catalog as absent instead of reading control metadata.
Fix: the GetControl handler's control lookup and the catalog seed state it reads in services/service-com-amazonaws-controlcatalog-controlcatalog - the control records must be keyed by the catalog ARN the request carries, and 404 must be reserved for ARNs genuinely absent from the catalog.

Cause 2 of 2 - ListControls returns placeholder controls instead of the catalog's (fixture controlcatalog-catalog-list-controls-aws-config-configrule, steps task#1 and task#2).
AWS: Smithy ListControls (POST /list-controls, paginated on NextToken/MaxResults with items Controls; Controls is a required list) returns the catalog's control summaries for the requested Filter, each carrying the catalog's own Arn, Aliases, Name, Description, Behavior, Severity, ParameterRequirementSummary, Implementation, CreateTime, GovernedResources and GovernedProviders.
Emulator: for the AWS::Config::ConfigRule filter the expected catalog entries are missing and an emulator-authored record is returned in their place. Concretely, for the control aliased CONFIG.ACCOUNT.DT.1 the catalog entry (Arn .../control/4x11tie7h25sws4g7ilakhcb7, Name as the catalog words it, ParameterRequirementSummary OPTIONAL) is absent while an extra entry with a differently shaped ARN (.../control/0d316cbc7af46ffa3560a5c2b), a rewritten Name and Description, ParameterRequirementSummary NONE and a different CreateTime is present. In the second cluster the entire expected page content is missing ($.Controls[0] onward), and that cluster spans 1285 affected steps of the fixture.
Why: a client paginating ListControls to discover controls, or joining ListControls output to GetControl/ListControlMappings by ARN, gets ARNs that exist nowhere else in the emulator and control metadata (severity, parameter requirement, description) that does not match AWS, so discovery and any ARN join silently fail.
Fix: the ListControls handler's catalog projection and the same catalog seed state - entries must be served from the real catalog dataset with the catalog's ARNs and field values, not synthesized per-process, and the filtered page content must match the catalog for the requested Implementation type.

Traces: controlcatalog-catalog-get-control#task#1 (20 steps clustered); controlcatalog-catalog-list-controls-aws-config-configrule#task#1 and #task#2.

## Evidence
Cause 1: GetControl - expected status 200, got 404; $.Arn, $.Name, $.Description, $.Behavior, $.Severity, $.RegionConfiguration, $.Implementation, $.ParameterRequirementSummary, $.Parameters, $.Aliases, $.CreateTime, $.GovernedResources, $.GovernedProviders all missing (expected the catalog control body, got None). Cluster: 20 steps, controlcatalog-catalog-get-control#task#1..#task#20.
Cause 2: ListControls - status 200 both sides, but $.Controls[Arn="arn:aws:controlcatalog:::control/4x11tie7h25sws4g7ilakhcb7"] missing (expected the catalog entry with Aliases ['CONFIG.ACCOUNT.DT.1'], Behavior DETECTIVE, Severity HIGH, ParameterRequirementSummary OPTIONAL, Implementation ACCOUNT_PART_OF_ORGANIZATIONS) while $.Controls[Arn="arn:aws:controlcatalog:::control/0d316cbc7af46ffa3560a5c2b"] is extra (expected None, got an emulator-authored record with ParameterRequirementSummary NONE and a rewritten Name/Description). Second cluster: $.Controls[0] missing (expected the catalog's first page entry, got None), 1285 affected steps.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus simparity-wire-aws-v1.
2. Replay fixture controlcatalog-catalog-get-control step task#1: POST /get-control with a catalog ControlArn -> emulator answers 404 with no control body; AWS answers 200 with the control.
3. Replay fixture controlcatalog-catalog-list-controls-aws-config-configrule steps task#1 and task#2: POST /list-controls filtered on Implementation type AWS::Config::ConfigRule -> the catalog's control entries are absent and an entry with an emulator-generated ARN is returned instead.
