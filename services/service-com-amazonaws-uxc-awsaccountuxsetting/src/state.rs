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
/// `ListServices` returns "Amazon Web Services service identifiers that you can
/// use as values for the visibleServices setting", so every entry here is an AWS
/// service's endpoint prefix / signing name and satisfies the model's `Service`
/// member constraints (`length 1..64`, `pattern ^[a-z0-9]+(-[a-z0-9]+)*$` — see
/// `smithy_query(query="shape", shape="ServiceList")`). The real catalog varies by
/// partition; this emulator serves one fixed, deterministic catalog.
///
/// It is a DISCOVERY list, not an admission list: this catalog tells a caller
/// which identifiers exist, and `UpdateAccountCustomizations` accepts any
/// `visibleServices` member its modelled constraints admit, whether or not this
/// list happens to name it (the catalog's own contents are the subject of their
/// own issue).
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
