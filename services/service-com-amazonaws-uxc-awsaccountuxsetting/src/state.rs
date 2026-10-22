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
    /// Store the visible-service identifiers a request supplied, in the order
    /// this setting reports them — see [`canonical_members`].
    pub fn set_visible_services(&mut self, supplied: Vec<String>) {
        self.visible_services = Some(canonical_members(supplied));
    }

    /// Store the visible-Region codes a request supplied, under the same rule as
    /// [`AccountUxCustomization::set_visible_services`]: one rule for both list
    /// settings, because the service keeps them alike.
    pub fn set_visible_regions(&mut self, supplied: Vec<String>) {
        self.visible_regions = Some(canonical_members(supplied));
    }
}

/// The wire value `AccountColor` uses for "no color set" — the documented default
/// `GetAccountCustomizations` reports for an account that never set one.
pub const ACCOUNT_COLOR_NONE: &str = "none";

/// The value a list setting holds after a request supplied `supplied`: the same
/// members, in one stable canonical order.
///
/// A `visibleServices` / `visibleRegions` setting names what is visible in the
/// console, so the ORDER a caller lists its members in carries no meaning to the
/// service: what it stores and reports is the members, and the order it reports
/// them in is its own — lexicographic ascending here, the canonical order for
/// identifiers and also the order the service's own `ListServices` catalog pages
/// in. Two requests naming the same members in different sequences therefore
/// leave the setting in the same state and answer identically, which is what
/// `@idempotent` means for a setting whose value is a collection.
///
/// Nothing else is changed: a member repeated in the request stays repeated,
/// because `ServiceList`/`RegionsList` are modelled as lists with no
/// `@uniqueItems` (`smithy_query(query="shape", shape="ServiceList,RegionsList")`),
/// so collapsing a duplicate would drop a value the model admits.
///
/// What is NOT reproducible is the exact sequence of one recorded response: the
/// corpus holds two responses describing the SAME members, with no write between
/// them, in opposite sequences. A per-response sequence AWS varies on its own is
/// not something an emulator can — or should — reproduce; the members, their type
/// and the stability across reads are the contract, and those are what this
/// function keeps.
pub fn canonical_members(supplied: Vec<String>) -> Vec<String> {
    let mut members = supplied;
    members.sort();
    members
}

/// The service identifiers `ListServices` enumerates, in the order the catalog is
/// seeded (sorted below, so paging is coherent).
///
/// `ListServices` returns "Amazon Web Services service identifiers that you can
/// use as values for the visibleServices setting", so every entry here is an AWS
/// service's endpoint prefix / signing name and satisfies the model's `Service`
/// member constraints (`length 1..64`, `pattern ^[a-z0-9]+(-[a-z0-9]+)*$` — see
/// `smithy_query(query="shape", shape="ServiceList")`). The real catalog varies by
/// partition; this emulator serves one fixed, deterministic catalog.
const SEED_SERVICE_CATALOG: &[&str] = &[
    "access-analyzer",
    "account",
    "acm",
    "amplify",
    "apigateway",
    "appconfig",
    "athena",
    "autoscaling",
    "backup",
    "batch",
    "bedrock",
    "cloudformation",
    "cloudfront",
    "cloudtrail",
    "cloudwatch",
    "codebuild",
    "cognito-idp",
    "config",
    "dynamodb",
    "ec2",
    "ecr",
    "ecs",
    "eks",
    "elasticache",
    "events",
    "firehose",
    "glue",
    "iam",
    "kinesis",
    "kms",
    "lambda",
    "logs",
    "organizations",
    "rds",
    "route53",
    "s3",
    "sagemaker",
    "secretsmanager",
    "sns",
    "sqs",
    "ssm",
    "states",
    "sts",
    "waf",
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

    /// Whether `service` names an entry of the catalog [`ListServices`] pages
    /// over — which is what makes it a valid `visibleServices` value, since the
    /// model documents that catalog as the source of valid identifiers.
    ///
    /// A READ: the catalog is fixed at construction, so membership never depends
    /// on who asked. Binary search is sound because [`Default`] sorts the
    /// catalog, which is also what makes paging over it coherent.
    ///
    /// [`ListServices`]: crate::ops::list_services::ListServices
    pub fn is_catalog_service(&self, service: &str) -> bool {
        debug_assert!(
            self.service_catalog.windows(2).all(|pair| pair[0] <= pair[1]),
            "the catalog must stay sorted for membership to be decidable by binary search"
        );
        self.service_catalog
            .binary_search_by(|entry| entry.as_str().cmp(service))
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
