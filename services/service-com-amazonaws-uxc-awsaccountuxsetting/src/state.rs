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
    /// Store the visible-service identifiers a request supplied, CANONICALIZED by
    /// [`canonical_members`]: the members, in ascending order of the identifier,
    /// whatever sequence the request named them in.
    pub fn set_visible_services(&mut self, supplied: Vec<String>) {
        self.visible_services = Some(canonical_members(supplied));
    }

    /// Store the visible-Region codes a request supplied, under the same rule as
    /// [`AccountUxCustomization::set_visible_services`]: one rule for both list
    /// settings, because the service keeps them alike.
    pub fn set_visible_regions(&mut self, supplied: Vec<String>) {
        self.visible_regions = Some(canonical_members(supplied));
    }

    /// Return the visible-service setting to its DEFAULT: unconfigured, which is
    /// what an account that never set it carries and what a read reports as an
    /// absent member ("visible Regions and visible services return null") — "all
    /// services visible", a state no stored list can express.
    ///
    /// Distinct from storing an EMPTY list, which is the setting configured to
    /// name no service: that is a present, empty array to every reader.
    pub fn reset_visible_services(&mut self) {
        self.visible_services = None;
    }

    /// Return the visible-Region setting to its default, under the same rule as
    /// [`AccountUxCustomization::reset_visible_services`].
    pub fn reset_visible_regions(&mut self) {
        self.visible_regions = None;
    }

    /// The `accountColor` this record is REPORTED with: the stored value, or
    /// [`ACCOUNT_COLOR_NONE`] while the account never configured one — "a value of
    /// `none` indicates that you have not set a color", so an unconfigured colour
    /// is a present member carrying that value and never an absent one.
    ///
    /// Stated HERE, beside the list settings' own shared rule, because every
    /// operation that renders a customization record reports the colour — the read
    /// and the update's echo of the record it just stored — and they must agree:
    /// the same stored record has to report the same colour whichever call a client
    /// asks through.
    pub fn reported_account_color(&self) -> &str {
        self.account_color.as_deref().unwrap_or(ACCOUNT_COLOR_NONE)
    }
}

/// The canonical sequence a list setting (`visibleServices` / `visibleRegions`) is
/// persisted, echoed and read back in: its members sorted into ascending order,
/// by the bytes of the identifier.
///
/// # Why an order of this service's choosing rather than the caller's
///
/// The model fixes no order: `ServiceList` and `RegionsList` are plain Smithy
/// lists with no `@uniqueItems` and no sort trait
/// (`smithy_query(query="shape", shape="ServiceList,RegionsList")`), and the
/// operation docs say only that the response carries "the current list of visible
/// service identifiers / Region codes after the update". So the order of these
/// arrays is RECORDED BEHAVIOUR ONLY, and what the corpus records is a service
/// that reports the setting in ascending member order rather than in the order a
/// request happened to use: the update of `uxc-configure-visible-regions-roundtrip`
/// is recorded answering the three Region codes ascending, and so is every other
/// recorded response for either setting — the
/// `uxc-configure-visible-services-roundtrip` echoes and reads, and the
/// `GetAccountCustomizations` of `uxc-partial-update-preserves-other-fields`.
///
/// Canonicalizing on the WRITE (rather than when serializing) is what makes the
/// update's own echo and every later `GetAccountCustomizations` report the one
/// same sequence, which is the property a round-trip client depends on; and it
/// keeps `@idempotent` exactly: re-applying a request leaves the same state and
/// answers the same body, and two requests naming the same members in different
/// sequences now converge on the same stored value instead of on two.
///
/// # What it does NOT do
///
/// It does not de-duplicate: neither list declares `@uniqueItems`, so a member
/// named twice is stored twice (adjacently, after the sort). The multiset of
/// members is the caller's; only their order is the service's.
///
/// # The one recorded response this does not reproduce
///
/// In `uxc-partial-update-preserves-other-fields` the update at `task#2` is
/// recorded reporting the two stored service identifiers in the one sequence that
/// is NOT ascending, while the `GetAccountCustomizations` at `task#4` — with no
/// write in between — reports the same two ascending, and that read is reproduced.
/// Two different orders for one stored value, so no rule that is a function of the
/// request and the stored record can answer both. Each candidate was measured
/// against the corpus rather than argued: reporting the members ascending
/// reproduces the read and not the echo; persisting and echoing the sequence the
/// request supplied (so that the echo is the caller's own document) leaves that
/// echo diverging at the same two positions, which is what rules the request's
/// sequence out as well; and the only sequence left for that step is the reverse
/// of both, which the other recorded update echoes contradict. That step's order
/// is per-response variation, and reproducing it would mean keying this write on
/// one recorded response. The canonical order below is the one that agrees with
/// every other recorded response, including that fixture's own later read.
pub fn canonical_members(mut supplied: Vec<String>) -> Vec<String> {
    supplied.sort();
    supplied
}

/// The wire value `AccountColor` uses for "no color set" — the documented default
/// an account that never set one is reported with, by
/// [`AccountUxCustomization::reported_account_color`], which is the one place that
/// rule is applied.
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
/// setting keys on are therefore the ones a console service HOME is addressed by —
/// the `console.aws.amazon.com/<identifier>` namespace — and NOT the set of SigV4
/// signing names / endpoint prefixes a client signs and routes with. The two
/// overlap (`s3`, `ec2`, `lambda` — the three the model's own `@examples` output
/// lists) but they are not the same set: this catalog names Amazon Cognito
/// `cognito` where the API endpoint prefix is `cognito-idp`, names CloudWatch Logs
/// under `cloudwatch` where the prefix is `logs`, names AWS Systems Manager
/// `systems-manager` where the prefix is `ssm`, Amazon EMR `emr` where the prefix
/// is `elasticmapreduce` and IAM Identity Center `singlesignon` where the prefix is
/// `sso`, and publishes console-only entries (`activate`, `artifact`,
/// `aws-marketplace`, `ce`) for services with no endpoint prefix of that name at
/// all. A catalog keyed on endpoint prefixes answers a discovery call in the wrong
/// namespace, so an identifier a console client may legitimately set looks
/// nonexistent.
///
/// Where a service has no console home of its own because the console files it
/// under another service's home, the filed service's own identifier is not an entry
/// and the HOME is: `access-analyzer` is not an entry and `iam` is.
///
/// # Which spelling of a service is the entry
///
/// Not a rule — the published catalog's own choice, service by service, so the
/// observation below decides it rather than a derivation from either namespace: the
/// catalog names Amazon MSK `kafka` and not `msk`, Amazon Location Service `geo` and
/// not `location`, AWS Health `health-dashboard` and not `health`, AWS Migration Hub
/// `mgh` and not `migrationhub`, and Amazon MQ `amazon-mq` (beside `mq`). A near
/// spelling the catalog does not publish is not an entry, so it is neither reported
/// by `ListServices` nor settable (`catalog_published_spellings` pins the spellings
/// and `console_identifier_namespace` the prefix contrast).
///
/// # Where these entries come from
///
/// Seeded from an OBSERVED catalog of that namespace — not independently derived:
/// the head of the list and entries such as `aws-agent-registry`, `bedrock-mantle`
/// and `claude-platform-on-aws` are identifiers only an observed catalog names, and
/// the rest are the identifiers AWS services' homes are published under. The same
/// observation is what adds `gameliftservers` and `gameliftstreams` (the two
/// GameLift homes beside the legacy `gamelift` one), `geo`, `health-dashboard`,
/// `infrastructure-composer`, `iq`, `kafka`, `managedservices`, `medical-imaging`,
/// `mgh`, `mq`, `network-security-manager`, `nova-act`, `odb`, `partnercentral` and
/// `prometheus`, and what leaves out identifiers it does not name (`greengrass`,
/// `health`, `iotanalytics`, `iotevents`, `iotwireless`, `kinesisanalytics`,
/// `location`, `lookoutmetrics`, `lookoutvision`, `migrationhub`, `msk`, `mwaa`,
/// `network-firewall`, `networkmanager`, `opensearch`, `opsworks`, `panorama`,
/// `pca-connector-ad`) however plausible a service name they are: a catalog seeded
/// from service names rather than from the published list reports identifiers no
/// client of the real service can discover, and admits them as settings.
/// Each entry satisfies the model's `Service` member constraints
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
/// Entries no service home is published under are deliberately absent —
/// `access-analyzer` (IAM Access Analyzer lives under `iam`), `account`,
/// `appconfig` (under `systems-manager`), `cognito-idp`, `kafkaconnect` (MSK
/// Connect lives under the MSK home, `kafka`), `logs` and `sts` — so the catalog is
/// one namespace throughout, and every home an omission above redirects to is
/// itself an entry a caller can discover and set.
///
/// It is the ADMISSION list as well as the discovery list: `ListServices`
/// "returns a paginated list of ... service identifiers that you can use as
/// values for the `visibleServices` setting", and the setting's own member doc
/// sends a caller here for "valid service identifiers", so an identifier this
/// catalog does not name is not a valid value of the setting and
/// `UpdateAccountCustomizations` refuses it ([`ComAmazonawsUxcAwsaccountuxsettingState::is_catalog_service`]).
/// The two lists being one is what makes the refusal workable: everything a
/// caller can discover here, it can set.
const SEED_SERVICE_CATALOG: &[&str] = &[
    "account-access",
    "acm",
    "acm-pca",
    "activate",
    "aidevops",
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
    "gameliftservers",
    "gameliftstreams",
    "geo",
    "globalaccelerator",
    "glue",
    "grafana",
    "groundstation",
    "guardduty",
    "health-dashboard",
    "healthlake",
    "iam",
    "imagebuilder",
    "infrastructure-composer",
    "inspector",
    "iot",
    "iotfleetwise",
    "iotsitewise",
    "iottwinmaker",
    "iq",
    "ivs",
    "kafka",
    "kendra",
    "keyspaces",
    "kinesis",
    "kinesisvideo",
    "kms",
    "lakeformation",
    "lambda",
    "launchwizard",
    "lex",
    "license-manager",
    "lightsail",
    "lookoutequipment",
    "m2",
    "machinelearning",
    "macie",
    "managedblockchain",
    "managedservices",
    "mediaconnect",
    "mediaconvert",
    "medialive",
    "mediapackage",
    "mediastore",
    "mediatailor",
    "medical-imaging",
    "memorydb",
    "mgh",
    "mgn",
    "monitron",
    "mq",
    "neptune",
    "network-security-manager",
    "notifications",
    "nova-act",
    "odb",
    "omics",
    "organizations",
    "outposts",
    "partnercentral",
    "payment-cryptography",
    "pcs",
    "personalize",
    "pinpoint",
    "polly",
    "prometheus",
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
    "singlesignon",
    "sns",
    "sqs",
    "states",
    "storagegateway",
    "supplychain",
    "support",
    "swf",
    "systems-manager",
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

    /// Whether `identifier` names an entry of the catalog `ListServices`
    /// enumerates — which is what makes it a valid `visibleServices` value.
    ///
    /// A binary search, which [`ComAmazonawsUxcAwsaccountuxsettingState::default`]
    /// makes valid by sorting and de-duplicating the catalog it builds; the same
    /// property page tokens depend on. Membership is exact: the catalog publishes
    /// one spelling of each identifier, so nothing is folded or trimmed here.
    pub fn is_catalog_service(&self, identifier: &str) -> bool {
        self.service_catalog
            .binary_search_by(|entry| entry.as_str().cmp(identifier))
            .is_ok()
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
