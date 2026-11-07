Issue-Source: trace-sweep-divergence
Issue-Trace: appconfig-catalog-list-extensions
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appconfig-catalog-list-extensions
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig|ListExtensions|2xx->2xx|$.Items[Arn="arn:aws:appconfig:us-west-2::extension/AWS.AppConfig.DeploymentEventsToSns/1"]
Issue-Key: appconfig|ListExtensions|2xx->2xx|$.Items[Arn="arn:aws:appconfig:us-west-2::extension/AWS.AppConfig.DeploymentEventsToSqs/1"]
Issue-Key: appconfig|ListExtensions|2xx->2xx|$.Items[Arn="arn:aws:appconfig:us-west-2::extension/AWS.AppConfig.DeploymentNotificationsToEventBridge/1"].Description
Issue-Key: appconfig|ListExtensions|2xx->2xx|$.Items[Arn="arn:aws:appconfig:us-west-2::extension/AWS.AppConfig.DeploymentNotificationsToSns/1"]
Issue-Key: appconfig|ListExtensions|2xx->2xx|$.Items[Arn="arn:aws:appconfig:us-west-2::extension/AWS.AppConfig.DeploymentNotificationsToSqs/1"]
Issue-Key: appconfig|ListExtensions|2xx->2xx|$.Items[Arn="arn:aws:appconfig:us-west-2::extension/AWS.AppConfig.FeatureFlags/1"].Description
Issue-Key: appconfig|ListExtensions|2xx->2xx|$.Items[Arn="arn:aws:appconfig:us-west-2::extension/AWS.AppConfig.JiraIntegration/1"].Description
Issue-Key: appconfig|ListExtensions|2xx->2xx|$.Items[Arn="arn:aws:appconfig:us-west-2::extension/AWS.Evidently.ProjectImporter/1"].Description
Issue-Commit: 49d4e81f9c14b9ea7dd829b0faa6f7c6d506b5c4

# appconfig ListExtensions serves a divergent AWS-authored extension catalog

> _Note: this issue was identified against the code base at commit `49d4e81f9c14b9ea7dd829b0faa6f7c6d506b5c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListExtensions

## Description
This issue carries 1 net-new cause in appconfig. The replay reports this cluster with stub_path empty and owned_by empty, i.e. no open issue carries its replay key.

Related open issue (named here because it describes part of the same surface but is NOT credited with this step by the replay's ownership report): 20261105T012540Z-appconfig-synthesized-response-content-diverges-deployment-eventlog-entries.md, whose Cause 1 covers only the Description *text* of the predefined extensions. The cluster replayed now is wider than that: besides the Description text, the catalog's ENTRY SET itself diverges (whole $.Items[...] entries, with no member suffix, are reported missing/extra), so the catalog seed must be corrected as a set, not only re-worded. If that issue is fixed by editing Description strings alone, this step still diverges.

Cause 1 of 1: ListExtensions returns a predefined (AWS-authored) extension catalog whose entry set and per-entry Description both differ from AWS's.
  AWS: Smithy: ListExtensions GET /extensions, http code 200, output Extensions{Items -> ExtensionSummaries of ExtensionSummary{Id (@pattern ^[a-z0-9]{4,7}$), Name (@length 1..64), VersionNumber (integer, default 0), Arn (@length 20..2048), Description (@length 0..1024, doc "Information about the extension")}}; errors BadRequestException (400), InternalServerException (500). The model does not enumerate the catalog itself, so for the catalog content this is recorded behavior only: on an account with no customer extensions, AWS returns a fixed set of AWS-authored extensions whose ARNs have the form arn:aws:appconfig:<region>::extension/<Name>/<VersionNumber> (no account id), and each entry carries a fixed, AWS-authored Description keyed by extension name and version. Both the membership of that set and each entry's Description are entirely fixed text — nothing in them comes from the request or from account state, so the only environment-specific part is the ARN's partition/region segment. The recording shows the set including AWS.AppConfig.FeatureFlags/1, AWS.AppConfig.JiraIntegration/1, AWS.Evidently.ProjectImporter/1, AWS.AppConfig.DeploymentNotificationsToEventBridge/1, AWS.AppConfig.DeploymentNotificationsToSns/1 and AWS.AppConfig.DeploymentNotificationsToSqs/1, each with a non-empty Description (e.g. FeatureFlags/1 = 'Validates AppConfig feature flag data automatically against a JSON schema that includes structure and constraints. Also transforms feature flag data prior to sending to the client. This extension is automatically associated to configuration profiles with type "AWS.AppConfig.FeatureFlags".'; DeploymentNotificationsToEventBridge/1 = 'Sends events to Amazon EventBridge when a deployment of configuration data in AppConfig is started, completed, or rolled back. Can be associated to the following resources in AppConfig: Application, Environment, Configuration Profile.'; JiraIntegration/1 = 'Exports feature flag data from AWS AppConfig into Jira. The lifecycle of each feature flag in AppConfig is tracked in Jira as an individual issue. Customers can see in Jira when flags are updated, turned on or off. Works in conjunction with the AppConfig app in the Atlassian Marketplace and is automatically associated to configuration profiles configured within that app.'; Evidently.ProjectImporter/1 = 'Imports Evidently Project data for use with its client-side evaluation feature, powered by AppConfig. This extension is automatically associated to ConfigurationProfiles created by Evidently.').
  Emulator: status matches at 200, but the body is a different catalog. (a) Entry set: the replay reports whole-item divergences, with no member path, at four deployment-notification ARNs — AWS.AppConfig.DeploymentNotificationsToSns/1 and AWS.AppConfig.DeploymentNotificationsToSqs/1 (reported 'missing': present in AWS's catalog, absent from the emulator's) alongside AWS.AppConfig.DeploymentEventsToSns/1 and AWS.AppConfig.DeploymentEventsToSqs/1, which do not occur in AWS's catalog at all. Hypothesis: the emulator's seed names the two deployment-notification extensions 'DeploymentEventsTo*' instead of AWS's 'DeploymentNotificationsTo*', so each AWS entry is reported missing and each emulator entry is an entry AWS never serves. (b) Description: where the emulator does carry the right ARN it serves its own shorter paraphrase instead of AWS's fixed string (FeatureFlags/1 comes back as 'Validates AppConfig feature flag data automatically against a JSON schema that includes the feature flag data format. Also transforms feature flag data prior to sending to the client.'; DeploymentNotificationsToEventBridge/1 as 'Sends AppConfig deployment events to the default Amazon EventBridge event bus'), and omits Description entirely (null) for AWS.AppConfig.JiraIntegration/1 and AWS.Evidently.ProjectImporter/1, where AWS always returns a string.
  Why: the predefined catalog is how a client discovers and then names an AWS-authored extension. A client that looks an extension up by Name or Arn to call GetExtension or CreateExtensionAssociation for the SNS/SQS deployment-notification extensions finds nothing in the emulator — the names AWS documents are simply not in the catalog — so the whole AWS-authored notification path is unreachable by its real identifier. A console or extension picker that renders Description shows different prose, or dereferences a null where the member is always populated. None of this is distinguishable by a client from 'this account has a different set of extensions'.
  Fix: the appconfig predefined/AWS-authored extension catalog seed data read by ListExtensions (and GetExtension / extension-association resolution), in the extension catalog/state component: the seeded entry set must be AWS's set under AWS's extension names, and every entry must carry AWS's verbatim Description text.
  Traces: appconfig-catalog-list-extensions#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1: 48 fixtures, 580 steps, 565 passed, 15 mismatched, score 97.4%, bootstrap_signal false. This cluster is reported with stub_path empty and owned_by empty (13 of the other 14 clusters are reported owned by existing open issues and are not refiled here).

Cluster: appconfig-catalog-list-extensions#task#1, operation ListExtensions, verdict mismatch, expected status 200 / actual status 200, affected_count 1.

Diverging keys reported in the cluster's divergence_key (all under $.Items, keyed by Arn):
  $.Items[Arn="arn:aws:appconfig:us-west-2::extension/AWS.AppConfig.DeploymentNotificationsToSns/1"]           — whole entry, no member suffix
  $.Items[Arn="arn:aws:appconfig:us-west-2::extension/AWS.AppConfig.DeploymentNotificationsToSqs/1"]           — whole entry; detail line begins 'missing'
  $.Items[Arn="arn:aws:appconfig:us-west-2::extension/AWS.AppConfig.DeploymentEventsToSns/1"]                  — whole entry, no member suffix
  $.Items[Arn="arn:aws:appconfig:us-west-2::extension/AWS.AppConfig.DeploymentEventsToSqs/1"]                  — whole entry, no member suffix
  $.Items[Arn=".../AWS.AppConfig.DeploymentNotificationsToEventBridge/1"].Description
  $.Items[Arn=".../AWS.AppConfig.FeatureFlags/1"].Description
  $.Items[Arn=".../AWS.AppConfig.JiraIntegration/1"].Description
  $.Items[Arn=".../AWS.Evidently.ProjectImporter/1"].Description

Detail lines (the replay's detail is elided mid-entry at the DeploymentNotificationsToSqs key):
  $.Items[...AWS.AppConfig.FeatureFlags/1].Description: value_mismatch — expected 'Validates AppConfig feature flag data automatically against a JSON schema that includes structure and constraints. Also transforms feature flag data prior to sending to the client. This extension is automatically associated to configuration profiles with type "AWS.AppConfig.FeatureFlags".', got 'Validates AppConfig feature flag data automatically against a JSON schema that includes the feature flag data format. Also transforms feature flag data prior to sending to the client.'
  $.Items[...AWS.AppConfig.JiraIntegration/1].Description: missing — expected 'Exports feature flag data from AWS AppConfig into Jira. The lifecycle of each feature flag in AppConfig is tracked in Jira as an individual issue. Customers can see in Jira when flags are updated, turned on or off. Works in conjunction with the AppConfig app in the Atlassian Marketplace and is automatically associated to configuration profiles configured within that app.', got None
  $.Items[...AWS.Evidently.ProjectImporter/1].Description: missing — expected 'Imports Evidently Project data for use with its client-side evaluation feature, powered by AppConfig. This extension is automatically associated to ConfigurationProfiles created by Evidently.', got None
  $.Items[...AWS.AppConfig.DeploymentNotificationsToEventBridge/1].Description: value_mismatch — expected 'Sends events to Amazon EventBridge when a deployment of configuration data in AppConfig is started, completed, or rolled back. Can be associated to the following resources in AppConfig: Application, Environment, Configuration Profile.', got 'Sends AppConfig deployment events to the default Amazon EventBridge event bus'
  $.Items[...AWS.AppConfig.DeploymentNotificationsToSqs/1]: missing (entry-level)

Fixed vs variable text: the per-extension Description strings above are entirely fixed text keyed by extension Name and VersionNumber — they contain no value taken from the request or from emulator state. In the ARNs, only the partition/region segment (here aws / us-west-2) is environment-specific; the ::extension/<Name>/<VersionNumber> tail, i.e. the extension's identity, is the requirement. Id values are system-generated per the model's ^[a-z0-9]{4,7}$ pattern and are not asserted.

Model cited: Smithy ListExtensions (GET /extensions, code 200, output Extensions.Items -> ExtensionSummaries) and ExtensionSummary{Id, Name, VersionNumber, Arn, Description @length 0..1024}, from services/service-com-amazonaws-appconfig-amazonappconfig/smithy-model/com-amazonaws-appconfig-amazonappconfig.json.

## Steps to Reproduce
1. Boot the emulator from a fresh state (no customer extensions created).
2. Call appconfig ListExtensions with no Name filter and no pagination (GET /extensions) and read $.Items.
3. Compare the entry set against AWS's AWS-authored catalog by the ::extension/<Name>/<Version> tail of each Arn: AWS.AppConfig.DeploymentNotificationsToSns/1 and AWS.AppConfig.DeploymentNotificationsToSqs/1 are absent from the emulator's response, while the emulator serves AWS.AppConfig.DeploymentEventsToSns/1 and AWS.AppConfig.DeploymentEventsToSqs/1, which AWS's catalog does not contain.
4. For the entries that are present, compare Description with AWS's fixed per-extension text: AWS.AppConfig.FeatureFlags/1 and AWS.AppConfig.DeploymentNotificationsToEventBridge/1 come back paraphrased, and AWS.AppConfig.JiraIntegration/1 and AWS.Evidently.ProjectImporter/1 come back with Description absent (null) where AWS always returns a string.
5. Try to use one of the missing entries the way a client would — call GetExtension / CreateExtensionAssociation naming the AWS extension AWS.AppConfig.DeploymentNotificationsToSqs — and observe that the identifier AWS documents resolves to nothing.
6. Replay fixture appconfig-catalog-list-extensions, step task#1, to confirm the cluster closes.
