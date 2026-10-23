// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-uxc-awsaccountuxsetting service.

use std::collections::HashMap;

/// One account's stored console customization record.
///
/// Each member is `Option` because the service distinguishes "never configured"
/// from "configured to this value": `GetAccountCustomizations` documents that an
/// unconfigured `visibleRegions`/`visibleServices` is returned as `null` (all
/// Regions / all services are visible) and an unconfigured `accountColor` as
/// `none`. An empty list is therefore NOT the same as an absent one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountUxCustomization {
    /// The stored `AccountColor` wire value, or `None` while never configured.
    pub account_color: Option<String>,
    /// The stored visible-service identifiers, or `None` while never configured.
    pub visible_services: Option<Vec<String>>,
    /// The stored visible-Region codes, or `None` while never configured.
    pub visible_regions: Option<Vec<String>>,
}

impl AccountUxCustomization {
    /// Store the visible-service identifiers a request supplied, as the exact
    /// sequence it supplied them in.
    ///
    /// A list setting's value is the caller's document. Neither the model nor the
    /// operation doc gives this service licence to re-order, de-duplicate or
    /// otherwise normalise it (`ServiceList`/`RegionsList` are plain lists with
    /// no `@uniqueItems` and no sort rule —
    /// `smithy_query(query="shape", shape="ServiceList,RegionsList")`), and the
    /// element order of a JSON array is part of the response document a client
    /// compares against what it sent or indexes positionally. So the sequence a
    /// request supplies is what is stored, what the update answers with and what
    /// `GetAccountCustomizations` reports afterwards — a round trip, not a
    /// rewrite.
    ///
    /// This stays consistent with `@idempotent`: replaying the SAME request
    /// leaves the same state and answers the same body. Two requests naming the
    /// same members in different sequences are different documents, and the
    /// service records whichever one arrived last.
    ///
    /// One recorded step disagrees, and it disagrees with ITSELF: in the corpus
    /// fixture `uxc-partial-update-preserves-other-fields`, the update at
    /// `task#2` is recorded with the two members in one sequence and the
    /// `GetAccountCustomizations` at `task#4` — with no write in between — with
    /// the same two members in the opposite sequence, so one of the two must
    /// disagree with whatever the request supplied. Measured, not assumed:
    /// storing the supplied sequence makes `task#4` agree and `task#2` differ,
    /// storing it reversed makes `task#2` agree and `task#4` differ, at the same
    /// two indices. Every other recorded response for this setting agrees with
    /// the supplied sequence, so that is the rule kept here; the one response
    /// that disagrees is per-response variation, and reproducing it would mean
    /// keying the write on the recording.
    pub fn set_visible_services(&mut self, supplied: Vec<String>) {
        self.visible_services = Some(supplied);
    }

    /// Store the visible-Region codes a request supplied, under the same rule as
    /// [`AccountUxCustomization::set_visible_services`]: one rule for both list
    /// settings, because the service keeps them alike.
    pub fn set_visible_regions(&mut self, supplied: Vec<String>) {
        self.visible_regions = Some(supplied);
    }
}

/// The wire value `AccountColor` uses for "no color set" — the documented default
/// `GetAccountCustomizations` reports for an account that never set one.
pub const ACCOUNT_COLOR_NONE: &str = "none";

/// The service identifiers `ListServices` enumerates, in the order the catalog is
/// seeded (sorted below, so paging is coherent).
///
/// # Which namespace this is
///
/// `ListServices` returns "a paginated list of Amazon Web Services service
/// identifiers that you can use as values for the `visibleServices` setting in
/// UpdateAccountCustomizations", and `visibleServices` "controls only the
/// appearance of services in the Amazon Web Services Management Console"
/// (`smithy_query(query="doc", shape="ListServices")`). The identifiers the
/// setting keys on are therefore the CONSOLE's own service identifiers — the
/// `console.aws.amazon.com/<identifier>` namespace a console service home is
/// addressed by — and NOT a service's SigV4 signing name / endpoint prefix, which
/// is the namespace a client signs and routes with. The two overlap (`s3`, `ec2`,
/// `lambda` — the three the model's own `@examples` output lists) but they are not
/// the same set: the console names Amazon Cognito `cognito` where the API endpoint
/// prefix is `cognito-idp`, names CloudWatch Logs under `cloudwatch` where the
/// prefix is `logs`, and publishes console-only entries (`activate`, `artifact`,
/// `aws-marketplace`, `ce`) for services with no endpoint prefix of that name at
/// all. A catalog keyed on endpoint prefixes answers a discovery call in the wrong
/// namespace, so an identifier a console client may legitimately set looks
/// nonexistent.
///
/// # Where these entries come from
///
/// Seeded from an OBSERVED console catalog of that namespace — not independently
/// derived: the head of the list and entries such as `aws-agent-registry`,
/// `bedrock-mantle` and `claude-platform-on-aws` are identifiers only an observed
/// catalog names, and the rest are the identifiers AWS services' console homes are
/// published under. Each entry satisfies the model's `Service` member constraints
/// (`@length(1, 64)`, `@pattern ^[a-z0-9]+(-[a-z0-9]+)*$` —
/// `smithy_query(query="shape", shape="ServiceList")`), which
/// `catalog_entries_are_legal_service_identifiers_in_one_stable_order` re-checks
/// over every entry rather than trusting this transcription.
///
/// AWS's own catalog "var[ies] by Amazon Web Services partition" (the operation
/// doc) and grows with every launch, so its exact membership on any given day is
/// not something a rule can derive: this emulator serves ONE fixed, deterministic
/// catalog of that namespace, large enough that paging over it is real (several
/// pages at the page sizes a client asks for) rather than a single short page.
/// Entries no console service home is published under are deliberately absent —
/// `access-analyzer` (IAM Access Analyzer lives under `iam`), `account`,
/// `appconfig` (under `systems-manager`), `cognito-idp`, `logs` and `sts` — so the
/// catalog is one namespace throughout.
///
/// It is a DISCOVERY list, not an admission list: this catalog tells a caller
/// which identifiers exist, and `UpdateAccountCustomizations` accepts any
/// `visibleServices` member its modelled constraints admit, whether or not this
/// list happens to name it.
const SEED_SERVICE_CATALOG: &[&str] = &[
    "account-access",
    "acm",
    "acm-pca",
    "activate",
    "aidevops",
    "airflow",
    "amazon-mq",
    "amplify",
    "apigateway",
    "appfabric",
    "appflow",
    "applicationrecoverycontroller",
    "appmesh",
    "apprunner",
    "appstream",
    "appstudio",
    "appsync",
    "artifact",
    "athena",
    "auditmanager",
    "autoscaling",
    "aws-agent-registry",
    "aws-marketplace",
    "awsforsap",
    "awsglobalview",
    "b2bi",
    "backup",
    "batch",
    "bedrock",
    "bedrock-agentcore",
    "bedrock-mantle",
    "billing",
    "billingconductor",
    "biodiscovery",
    "braket",
    "budgets",
    "ce",
    "chatbot",
    "chime",
    "chimesdk",
    "claude-platform-on-aws",
    "cleanrooms",
    "cloud9",
    "clouddirectory",
    "cloudformation",
    "cloudfront",
    "cloudhsm",
    "cloudsearch",
    "cloudshell",
    "cloudtrail",
    "cloudwatch",
    "codeartifact",
    "codebuild",
    "codecatalyst",
    "codecommit",
    "codedeploy",
    "codeguru",
    "codepipeline",
    "cognito",
    "compute-optimizer",
    "config",
    "connect",
    "controltower",
    "databrew",
    "dataexchange",
    "datapipeline",
    "datasync",
    "datazone",
    "deadline-cloud",
    "deepracer",
    "detective",
    "devicefarm",
    "devops-guru",
    "directconnect",
    "directoryservice",
    "dms",
    "docdb",
    "drs",
    "dsql",
    "dynamodb",
    "ec2",
    "ecr",
    "ecs",
    "efs",
    "eks",
    "elasticache",
    "elasticbeanstalk",
    "elastictranscoder",
    "emr",
    "entityresolution",
    "events",
    "evs",
    "finspace",
    "firehose",
    "fis",
    "fms",
    "forecast",
    "frauddetector",
    "freertos",
    "fsx",
    "gamelift",
    "globalaccelerator",
    "glue",
    "grafana",
    "greengrass",
    "groundstation",
    "guardduty",
    "health",
    "healthlake",
    "iam",
    "imagebuilder",
    "inspector",
    "iot",
    "iotanalytics",
    "iotevents",
    "iotfleetwise",
    "iotsitewise",
    "iottwinmaker",
    "iotwireless",
    "ivs",
    "kafka",
    "kafkaconnect",
    "kendra",
    "keyspaces",
    "kinesis",
    "kinesisanalytics",
    "kinesisvideo",
    "kms",
    "lakeformation",
    "lambda",
    "launchwizard",
    "lex",
    "license-manager",
    "lightsail",
    "location",
    "lookoutequipment",
    "lookoutmetrics",
    "lookoutvision",
    "m2",
    "machinelearning",
    "macie",
    "managedblockchain",
    "mediaconnect",
    "mediaconvert",
    "medialive",
    "mediapackage",
    "mediastore",
    "mediatailor",
    "memorydb",
    "mgn",
    "migrationhub",
    "monitron",
    "neptune",
    "network-firewall",
    "networkmanager",
    "notifications",
    "omics",
    "opensearch",
    "opsworks",
    "organizations",
    "outposts",
    "panorama",
    "payment-cryptography",
    "pca-connector-ad",
    "pcs",
    "personalize",
    "pinpoint",
    "polly",
    "proton",
    "q",
    "qbusiness",
    "qldb",
    "quicksight",
    "ram",
    "rbin",
    "rds",
    "redshift",
    "redshift-serverless",
    "rekognition",
    "resiliencehub",
    "resource-explorer",
    "resource-groups",
    "robomaker",
    "rolesanywhere",
    "route53",
    "route53resolver",
    "s3",
    "sagemaker",
    "scheduler",
    "secretsmanager",
    "securityhub",
    "securitylake",
    "serverlessrepo",
    "servicecatalog",
    "servicequotas",
    "ses",
    "shield",
    "signer",
    "simspaceweaver",
    "sns",
    "sqs",
    "ssm",
    "sso",
    "states",
    "storagegateway",
    "supplychain",
    "support",
    "swf",
    "textract",
    "timestream",
    "transcribe",
    "transfer",
    "translate",
    "trustedadvisor",
    "verifiedpermissions",
    "vpc",
    "vpc-lattice",
    "waf",
    "wafv2",
    "wellarchitected",
    "wickr",
    "workdocs",
    "workmail",
    "workspaces",
    "workspaces-web",
    "xray",
];

/// Per-service state for com-amazonaws-uxc-awsaccountuxsetting. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsUxcAwsaccountuxsettingState {
    /// The customization record of each account that has one, keyed by the
    /// calling account's id. A missing key is the default (unconfigured) record.
    pub customizations: HashMap<String, AccountUxCustomization>,
    /// The ordered catalog `ListServices` pages over. Sorted and de-duplicated at
    /// construction so a `nextToken` resolved against it names a stable position.
    pub service_catalog: Vec<String>,
}

impl Default for ComAmazonawsUxcAwsaccountuxsettingState {
    fn default() -> Self {
        let mut service_catalog: Vec<String> =
            SEED_SERVICE_CATALOG.iter().map(|s| (*s).to_string()).collect();
        // Stable ordering is what makes pagination coherent: an offset-bearing
        // token must name the same element on every page request.
        service_catalog.sort();
        service_catalog.dedup();
        Self {
            customizations: HashMap::new(),
            service_catalog,
        }
    }
}

impl ComAmazonawsUxcAwsaccountuxsettingState {
    /// The customization stored for `account_id`, or the default record when that
    /// account has never been updated. A READ: it never inserts the key.
    pub fn customization(&self, account_id: &str) -> AccountUxCustomization {
        self.customizations
            .get(account_id)
            .cloned()
            .unwrap_or_default()
    }

    /// The customization record for `account_id`, created empty on first write.
    pub fn customization_mut(&mut self, account_id: &str) -> &mut AccountUxCustomization {
        self.customizations.entry(account_id.to_string()).or_default()
    }
}

/// The account a request is made by, which keys [`ComAmazonawsUxcAwsaccountuxsettingState::customizations`].
///
/// Customizations are per account, and this emulator presents exactly one
/// canonical account (`engine::emulator_account_id`) whatever credentials sign the
/// request — so every caller resolves to that account rather than to its access
/// key id. Resolved through this accessor, never a literal, so the key follows the
/// emulator's account contract.
pub fn calling_account_id() -> &'static str {
    ::engine::emulator_account_id()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsUxcAwsaccountuxsettingState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsUxcAwsaccountuxsettingState>)
}
