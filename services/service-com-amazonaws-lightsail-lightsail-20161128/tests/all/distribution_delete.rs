// SPDX-License-Identifier: CC0-1.0

//! Deleting a Lightsail CDN distribution, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The harness is the one `instance_lifecycle.rs` already has.
//!
//! The rule under test: `DeleteDistribution` removes the ONE stored distribution
//! its name addresses together with everything that lives on that record — its
//! origin, its bundle, its cache configuration, its tags, the certificate it
//! named and the last reset of its cached content — so no later read reports it,
//! its name is free for a distribution that inherits nothing from it, and the
//! call answers one terminal `DeleteDistribution` operation-log entry located in
//! the one Region a distribution lives in. Nothing else is touched: the origin
//! resource it pulled from, a certificate it served HTTPS traffic with (released,
//! not deleted) and every other distribution of the account all survive it. A
//! name that names no distribution — including one deleted a moment ago — is the
//! service's `DoesNotExist` rejection rather than a reported deletion that did
//! not happen, and the call is served by the N. Virginia (`us-east-1`) endpoint
//! ALONE, as the modelled `InvalidInputException` documentation states for the
//! domain and distribution APIs together.
//!
//! So the cases are TABLES: one row per published bundle plan crossed with the
//! kind of origin the distribution pulls from, how much optional configuration
//! it carries, whether a certificate is attached, whether its cache was reset,
//! whether it has finished deploying when the delete is made and whether another
//! distribution stands beside it; the refusals are a table over the kinds of name
//! that address no distribution; and the Region restriction is a table over every
//! Region the modelled `RegionName` enum lists that is not the distribution
//! Region, plus one it does not list.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::ops::types::REGION_NAME_VALUES;

// The harness, the wire error code, the minted-id check, the instance stand-up
// and the distribution and certificate readers this test target already exposes:
// a sibling suite drives the service through the same front door rather than
// standing up a second one.
use crate::disk_attachment::create_running_instance;
use crate::distribution_certificate_attachment::{
    certificate, create_certificate, distribution, wait_for_distribution_status,
    wait_for_issued_certificate,
};
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The operation under test, by the name its `X-Amz-Target` carries.
const DELETE: &str = "DeleteDistribution";

/// The `operationType` the log entry of a distribution's removal carries, the
/// modelled `OperationType` enum's own `DeleteDistribution` member
/// (`smithy_query(query="shape",shape="OperationType")`).
const OPERATION_TYPE: &str = "DeleteDistribution";

/// The `resourceType` the entry names, the modelled `ResourceType` enum's own
/// `Distribution` member.
const RESOURCE_TYPE: &str = "Distribution";

/// The terminal `Operation$status` of work that was already done when the call
/// returned.
const SUCCEEDED: &str = "Succeeded";

/// The one Region a distribution lives in, from the modelled
/// `LightsailDistribution$location` documentation ("all distributions are
/// located in the `us-east-1` Region").
const DISTRIBUTION_REGION: &str = "us-east-1";

/// The Availability Zone an origin instance is created in: the first zone of the
/// distribution Region.
const ORIGIN_ZONE: &str = "us-east-1a";

/// The `status` a distribution reports once its configuration has rolled out,
/// and the one it reports while a rollout is outstanding
/// (`crate::state::DISTRIBUTION_STATUS_DEPLOYED` /
/// `crate::state::DISTRIBUTION_STATUS_IN_PROGRESS`).
const DEPLOYED: &str = "Deployed";
/// See [`DEPLOYED`].
const IN_PROGRESS: &str = "InProgress";

/// The wire error code (awsJson `__type`) of Lightsail's rejection of a name
/// that addresses nothing, and the `code` its body carries — which a Lightsail
/// SDK client branches on rather than on the HTTP status.
const NOT_FOUND: &str = "NotFoundException";
/// See [`NOT_FOUND`].
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// The wire error code of a refusal of a request the model admits but the
/// service does not serve.
const INVALID_INPUT: &str = "InvalidInputException";

/// The refusal the modelled `InvalidInputException` documentation states
/// verbatim, so its text is a requirement and not an example:
/// `smithy_query(query="doc",shape="InvalidInputException")` reads "Domain and
/// distribution APIs are only available in the N. Virginia (us-east-1) Amazon
/// Web Services Region. Please set your Amazon Web Services Region configuration
/// to us-east-1 to create, view, or edit these resources."
const REGION_REFUSAL: &str = "Domain and distribution APIs are only available in the N. Virginia \
                              (us-east-1) Amazon Web Services Region. Please set your Amazon Web \
                              Services Region configuration to us-east-1 to create, view, or \
                              edit these resources.";

/// An Amazon Web Services Region the modelled `RegionName` enum does NOT list,
/// so a case can sit outside the catalog: a scope naming a Region Lightsail does
/// not serve at all is no more the distribution Region than any other.
const OFF_ENUM_REGION: &str = "il-central-1";

/// An upper bound on the pages one listing walk may take, so a broken paging
/// contract fails the assertion rather than looping forever.
const MAX_PAGES: usize = 20;

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// What kind of Lightsail resource a distribution's origin is, from the modelled
/// `Origin` documentation: "An origin can be a Lightsail instance, bucket,
/// container service, or load balancer."
#[derive(Clone, Copy, Debug)]
enum OriginKind {
    Instance,
    Bucket,
}

/// Stand up the origin resource `name` of this kind in the distribution Region.
async fn create_origin(emulator: &Emulator, kind: OriginKind, name: &str) {
    match kind {
        OriginKind::Instance => create_running_instance(emulator, name, ORIGIN_ZONE).await,
        OriginKind::Bucket => {
            emulator
                .ok(
                    "CreateBucket",
                    json!({"bucketName": name, "bundleId": "small_1_0"}),
                )
                .await;
        }
    }
}

/// Require that the origin resource `name` of this kind survived the delete of
/// the distribution that pulled from it.
async fn assert_origin_survives(emulator: &Emulator, case: &str, kind: OriginKind, name: &str) {
    match kind {
        OriginKind::Instance => {
            let instance = emulator
                .ok("GetInstance", json!({"instanceName": name}))
                .await["instance"]
                .clone();
            assert_eq!(
                instance["name"].as_str(),
                Some(name),
                "case {case}: expected the origin instance {name} to survive, got {instance:?}"
            );
        }
        OriginKind::Bucket => {
            let body = emulator.ok("GetBuckets", json!({"bucketName": name})).await;
            let reported = body["buckets"]
                .as_array()
                .and_then(|buckets| buckets.first())
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                reported["name"].as_str(),
                Some(name),
                "case {case}: expected the origin bucket {name} to survive, got {body:?}"
            );
        }
    }
}

/// Every distribution name `GetDistributions` reports, walking every page.
async fn listed_names(emulator: &Emulator) -> Vec<String> {
    let mut names = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let request = match &token {
            Some(token) => json!({"pageToken": token}),
            None => json!({}),
        };
        let page = emulator.ok("GetDistributions", request).await;
        for reported in page["distributions"].as_array().unwrap_or_else(|| {
            panic!("case listing distributions: expected a distributions array, got {page:?}")
        }) {
            names.push(reported["name"].as_str().unwrap_or("<no name>").to_string());
        }
        match page["nextPageToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return names,
        }
    }
    panic!("case listing distributions: the paged walk did not terminate within {MAX_PAGES} pages");
}

/// The message Lightsail's `DoesNotExist` envelope carries for a name this
/// account holds no resource of that type under: the resource's TYPE and the
/// identifier the request carried
/// (`crate::scaffolded::Scaffolded::resource_does_not_exist`).
fn does_not_exist_message(name: &str) -> String {
    format!("The {RESOURCE_TYPE} does not exist: {name}")
}

/// Require that `name` names no distribution of this account: every reader of a
/// distribution answers HTTP 400, the wire error `NotFoundException` and
/// `DoesNotExist` in the body, with a message naming the resource type and the
/// requested name.
async fn assert_does_not_exist(emulator: &Emulator, case: &str, name: &str) {
    for operation in ["GetDistributions", "GetDistributionLatestCacheReset"] {
        let (status, body) = emulator
            .call(operation, json!({"distributionName": name}))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400 from {operation} for {name}, got {status} {body:?}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, NOT_FOUND,
            "case {case}: expected {NOT_FOUND} from {operation} for {name}, got {code} {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some(DOES_NOT_EXIST),
            "case {case}: expected the body code {DOES_NOT_EXIST} from {operation}, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(does_not_exist_message(name).as_str()),
            "case {case}: expected the refusal from {operation} to name the resource type and \
             {name}, got {body:?}"
        );
    }
}

/// The names of the strings in the JSON list `value`.
fn string_list(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The delete table
// ---------------------------------------------------------------------------

/// One distribution to create and then delete.
///
/// The rows differ along everything the rule generalises over: the published
/// bundle plan the distribution runs on, the kind of resource it pulls content
/// from, how much optional configuration it carries (tags, per-path cache
/// behaviours, a default root object, an IP address type), whether a certificate
/// is attached to it, whether its cache has been reset, whether it has finished
/// deploying when the delete is made, and whether another distribution stands
/// beside it.
struct Row {
    case: &'static str,
    name: &'static str,
    /// A bundle id from the service's published CDN plans
    /// (`GetDistributionBundles`), one row per plan.
    bundle_id: &'static str,
    origin_kind: OriginKind,
    origin_name: &'static str,
    tags: &'static [(&'static str, &'static str)],
    /// `(path, behavior)` per-path cache behaviours the distribution carries.
    cache_behaviors: &'static [(&'static str, &'static str)],
    default_root_object: Option<&'static str>,
    /// A value of the modelled `IpAddressType` enum, or `None` to take the
    /// documented default.
    ip_address_type: Option<&'static str>,
    /// `(certificateName, domainName, subjectAlternativeNames)` of a certificate
    /// attached to the distribution, or `None` for one serving no HTTPS traffic.
    certificate: Option<(&'static str, &'static str, &'static [&'static str])>,
    /// Whether the cached content is reset before the delete, so the record
    /// carries a cache reset the removal has to take with it.
    reset_cache: bool,
    /// Whether the distribution is waited into [`DEPLOYED`] before the delete: a
    /// delete does not wait for the rollout the clock owes a new record.
    wait_deployed: bool,
    /// Another distribution of the account, which the delete must leave exactly
    /// as it is.
    neighbour: Option<&'static str>,
}

/// The table.
const ROWS: &[Row] = &[
    Row {
        case: "the cheapest plan, an instance origin, nothing optional, deleted mid-rollout",
        name: "bare-cdn",
        bundle_id: "small_1_0",
        origin_kind: OriginKind::Instance,
        origin_name: "del-origin-1",
        tags: &[],
        cache_behaviors: &[],
        default_root_object: None,
        ip_address_type: None,
        certificate: None,
        reset_cache: false,
        wait_deployed: false,
        neighbour: None,
    },
    Row {
        case: "a bucket origin, one tag, a reset cache, a neighbour beside it",
        name: "bucket-cdn",
        bundle_id: "medium_1_0",
        origin_kind: OriginKind::Bucket,
        origin_name: "del-origin-bucket",
        tags: &[("env", "test")],
        cache_behaviors: &[],
        default_root_object: Some("index.html"),
        ip_address_type: Some("ipv4"),
        certificate: None,
        reset_cache: true,
        wait_deployed: true,
        neighbour: Some("bucket-keep-cdn"),
    },
    Row {
        case: "the priciest plan, per-path cache behaviours and an attached certificate",
        name: "cert-cdn",
        bundle_id: "large_1_0",
        origin_kind: OriginKind::Instance,
        origin_name: "del-origin-2",
        tags: &[("env", "prod"), ("team", "payments"), ("tier", "edge")],
        cache_behaviors: &[("/static/*", "cache"), ("/api/*", "dont-cache")],
        default_root_object: None,
        ip_address_type: Some("dualstack"),
        certificate: Some(("del-site-cert", "example.com", &["www.example.com"])),
        reset_cache: true,
        wait_deployed: true,
        neighbour: Some("cert-keep-cdn"),
    },
    Row {
        case: "a certificate attached and the rollout still outstanding when the delete lands",
        name: "rolling-cdn",
        bundle_id: "small_1_0",
        origin_kind: OriginKind::Bucket,
        origin_name: "del-origin-bucket-2",
        tags: &[("owner", "team-a")],
        cache_behaviors: &[("/assets/*", "cache")],
        default_root_object: Some("home.html"),
        ip_address_type: None,
        certificate: Some((
            "del-multi-cert",
            "shop.example.com",
            &["cart.example.com", "pay.example.com"],
        )),
        reset_cache: false,
        wait_deployed: false,
        neighbour: None,
    },
];

impl Row {
    /// This row as the `CreateDistribution` request body.
    fn create_request(&self) -> Value {
        let mut body = json!({
            "distributionName": self.name,
            "bundleId": self.bundle_id,
            "origin": {
                "name": self.origin_name,
                "regionName": DISTRIBUTION_REGION,
                "protocolPolicy": "http-only",
            },
            "defaultCacheBehavior": {"behavior": "cache"},
        });
        let map = body.as_object_mut().expect("the body is a JSON object");
        if !self.tags.is_empty() {
            map.insert(
                "tags".to_string(),
                Value::Array(
                    self.tags
                        .iter()
                        .map(|(key, value)| json!({"key": key, "value": value}))
                        .collect(),
                ),
            );
        }
        if !self.cache_behaviors.is_empty() {
            map.insert(
                "cacheBehaviors".to_string(),
                Value::Array(
                    self.cache_behaviors
                        .iter()
                        .map(|(path, behavior)| json!({"path": path, "behavior": behavior}))
                        .collect(),
                ),
            );
        }
        if let Some(object) = self.default_root_object {
            map.insert("defaultRootObject".to_string(), json!(object));
        }
        if let Some(kind) = self.ip_address_type {
            map.insert("ipAddressType".to_string(), json!(kind));
        }
        body
    }
}

/// Create the distribution `name` on `bundle_id`, pulling from `origin_name`.
async fn create_distribution(
    emulator: &Emulator,
    name: &str,
    bundle_id: &str,
    origin_name: &str,
) -> Value {
    emulator
        .ok(
            "CreateDistribution",
            json!({
                "distributionName": name,
                "bundleId": bundle_id,
                "origin": {
                    "name": origin_name,
                    "regionName": DISTRIBUTION_REGION,
                    "protocolPolicy": "http-only",
                },
                "defaultCacheBehavior": {"behavior": "cache"},
            }),
        )
        .await
}

/// RULE: `DeleteDistribution` removes the stored distribution its name addresses
/// with everything that lives on that record, answering one terminal
/// operation-log entry located where the distribution was; the record is then
/// gone from every read, its name is free for a distribution that inherits
/// nothing from it, and every resource beside it is untouched.
#[tokio::test]
async fn deleting_a_distribution_removes_it_and_everything_it_held() {
    let emulator = Emulator::new();
    let mut operation_ids: BTreeSet<String> = BTreeSet::new();

    for row in ROWS {
        let case = row.case;
        create_origin(&emulator, row.origin_kind, row.origin_name).await;
        if let Some(neighbour) = row.neighbour {
            create_distribution(&emulator, neighbour, "small_1_0", row.origin_name).await;
        }
        emulator.ok("CreateDistribution", row.create_request()).await;
        if let Some((certificate_name, domain, alternatives)) = row.certificate {
            create_certificate(&emulator, certificate_name, domain, alternatives).await;
            wait_for_issued_certificate(&emulator, certificate_name).await;
            emulator
                .ok(
                    "AttachCertificateToDistribution",
                    json!({"distributionName": row.name, "certificateName": certificate_name}),
                )
                .await;
        }
        if row.reset_cache {
            emulator
                .ok("ResetDistributionCache", json!({"distributionName": row.name}))
                .await;
        }
        if row.wait_deployed {
            wait_for_distribution_status(&emulator, row.name, DEPLOYED).await;
        }

        // What the record reports BEFORE the delete, so the assertions below
        // compare against the distribution that actually existed.
        let before = distribution(&emulator, row.name).await;
        let before_arn = before["arn"].as_str().unwrap_or_default().to_string();
        let before_domain = before["domainName"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            before["bundleId"].as_str(),
            Some(row.bundle_id),
            "case {case}: expected the created bundle to be held, got {before:?}"
        );
        assert_eq!(
            before["tags"].as_array().map(Vec::len),
            Some(row.tags.len()),
            "case {case}: expected {} tags on the record, got {before:?}",
            row.tags.len()
        );
        assert_eq!(
            before["cacheBehaviors"].as_array().map(Vec::len),
            Some(row.cache_behaviors.len()),
            "case {case}: expected {} cache behaviours on the record, got {before:?}",
            row.cache_behaviors.len()
        );
        assert_eq!(
            before["certificateName"].as_str(),
            row.certificate.map(|(name, _, _)| name),
            "case {case}: expected the attached certificate to be held, got {before:?}"
        );
        let expected_status = if row.wait_deployed {
            DEPLOYED
        } else {
            IN_PROGRESS
        };
        assert_eq!(
            before["status"].as_str(),
            Some(expected_status),
            "case {case}: expected the distribution to be {expected_status}, got {before:?}"
        );
        let neighbour_before = match row.neighbour {
            Some(neighbour) => Some(distribution(&emulator, neighbour).await),
            None => None,
        };

        let answered = emulator
            .ok(DELETE, json!({"distributionName": row.name}))
            .await;

        // The output carries a single `operation`, not a list: exactly one
        // distribution was deleted.
        let operation = answered["operation"].clone();
        assert!(
            operation.is_object(),
            "case {case}: expected one operation object, got {answered:?}"
        );
        assert_eq!(
            operation["operationType"].as_str(),
            Some(OPERATION_TYPE),
            "case {case}: expected the {OPERATION_TYPE} operation type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceType"].as_str(),
            Some(RESOURCE_TYPE),
            "case {case}: expected the {RESOURCE_TYPE} resource type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceName"].as_str(),
            Some(row.name),
            "case {case}: expected the entry to name {}, got {operation:?}",
            row.name
        );
        // The record is gone as the call returns, so there is nothing left for
        // the clock to owe this entry.
        assert_eq!(
            operation["status"].as_str(),
            Some(SUCCEEDED),
            "case {case}: expected a {SUCCEEDED} operation, got {operation:?}"
        );
        assert_eq!(
            operation["isTerminal"].as_bool(),
            Some(true),
            "case {case}: expected a terminal operation, got {operation:?}"
        );
        // A distribution is a global resource located in the one Region every
        // distribution lives in, and names no zone.
        assert_eq!(
            operation["location"]["regionName"].as_str(),
            Some(DISTRIBUTION_REGION),
            "case {case}: expected the entry in {DISTRIBUTION_REGION}, got {operation:?}"
        );
        assert_eq!(
            operation["location"]["availabilityZone"].as_str(),
            None,
            "case {case}: expected no availabilityZone on a global resource, got {operation:?}"
        );
        let id = operation["id"].as_str().unwrap_or_default().to_string();
        assert!(
            is_uuid_shaped(&id),
            "case {case}: expected a uuid-shaped operation id, got {id:?}"
        );
        assert!(
            operation_ids.insert(id.clone()),
            "case {case}: expected a fresh operation id, got the already-used {id:?}"
        );
        // The entry was APPENDED to the operation log, not synthesised for this
        // response: the log reports the same entry under the same id, and the
        // log of a deleted resource survives it.
        let logged = emulator
            .ok("GetOperation", json!({"operationId": id}))
            .await["operation"]
            .clone();
        assert_eq!(
            logged, operation,
            "case {case}: expected the log to hold the answered entry, got {logged:?}"
        );
        let for_resource = emulator
            .ok(
                "GetOperationsForResource",
                json!({"resourceName": row.name}),
            )
            .await;
        let delete_entries = for_resource["operations"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .filter(|entry| entry["operationType"].as_str() == Some(OPERATION_TYPE))
                    .count()
            })
            .unwrap_or_default();
        assert_eq!(
            delete_entries, 1,
            "case {case}: expected the resource's log to hold the one delete, got \
             {for_resource:?}"
        );

        // The record is gone from every read of it, and from the listing.
        assert_does_not_exist(&emulator, case, row.name).await;
        let listed = listed_names(&emulator).await;
        assert!(
            !listed.contains(&row.name.to_string()),
            "case {case}: expected the listing not to report {}, got {listed:?}",
            row.name
        );
        // A second delete fails rather than reporting a deletion that did not
        // happen.
        let (status, body) = emulator
            .call(DELETE, json!({"distributionName": row.name}))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a second delete to be refused, got {status} {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some(DOES_NOT_EXIST),
            "case {case}: expected the body code {DOES_NOT_EXIST} on a second delete, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(does_not_exist_message(row.name).as_str()),
            "case {case}: expected the second delete to name the resource type and {}, got \
             {body:?}",
            row.name
        );

        // The origin the distribution pulled from is a resource of its own and
        // survives it.
        assert_origin_survives(&emulator, case, row.origin_kind, row.origin_name).await;
        // So does a certificate it served HTTPS traffic with: the delete
        // RELEASES the attachment rather than deleting the certificate, so the
        // certificate is still the account's and counts one resource fewer.
        if let Some((certificate_name, domain, alternatives)) = row.certificate {
            let reported = certificate(&emulator, certificate_name).await;
            assert_eq!(
                reported["certificateName"].as_str(),
                Some(certificate_name),
                "case {case}: expected {certificate_name} to survive the distribution, got \
                 {reported:?}"
            );
            assert_eq!(
                reported["domainName"].as_str(),
                Some(domain),
                "case {case}: expected the certificate's domain to be unchanged, got {reported:?}"
            );
            let covered = string_list(&reported["certificateDetail"]["subjectAlternativeNames"]);
            for alternative in alternatives {
                assert!(
                    covered.iter().any(|name| name == alternative),
                    "case {case}: expected the certificate to still cover {alternative}, got \
                     {covered:?}"
                );
            }
            assert_eq!(
                reported["certificateDetail"]["inUseResourceCount"].as_i64(),
                Some(0),
                "case {case}: expected the certificate to be in use by nothing, got {reported:?}"
            );
        }
        // So does another distribution of the account, unchanged.
        if let (Some(neighbour), Some(expected)) = (row.neighbour, neighbour_before) {
            let after = distribution(&emulator, neighbour).await;
            assert_eq!(
                after, expected,
                "case {case}: expected {neighbour} to be untouched, got {after:?}"
            );
        }

        // The name is free again, and what the new record holds comes from the
        // new request alone: no tag, no cache behaviour, no certificate, no
        // cache reset and no server-assigned member of the record that is gone.
        create_distribution(&emulator, row.name, "small_1_0", row.origin_name).await;
        let reborn = distribution(&emulator, row.name).await;
        assert_eq!(
            reborn["bundleId"].as_str(),
            Some("small_1_0"),
            "case {case}: expected the new request's bundle, got {reborn:?}"
        );
        assert_eq!(
            reborn["tags"].as_array().map(Vec::len),
            Some(0),
            "case {case}: expected the new distribution to carry no tag, got {reborn:?}"
        );
        assert_eq!(
            reborn["cacheBehaviors"].as_array().map(Vec::len),
            Some(0),
            "case {case}: expected the new distribution to hold no cache behaviour, got {reborn:?}"
        );
        assert_eq!(
            reborn["certificateName"],
            Value::Null,
            "case {case}: expected the new distribution to hold no certificate, got {reborn:?}"
        );
        assert!(
            string_list(&reborn["alternativeDomainNames"]).is_empty(),
            "case {case}: expected the new distribution to publish no alternate domain, got \
             {reborn:?}"
        );
        assert_eq!(
            reborn["defaultRootObject"],
            Value::Null,
            "case {case}: expected the new distribution to hold no default root object, got \
             {reborn:?}"
        );
        assert_ne!(
            reborn["arn"].as_str(),
            Some(before_arn.as_str()),
            "case {case}: expected a freshly minted ARN, got {reborn:?}"
        );
        assert_ne!(
            reborn["domainName"].as_str(),
            Some(before_domain.as_str()),
            "case {case}: expected a freshly minted domainName, got {reborn:?}"
        );
        let reset = emulator
            .ok(
                "GetDistributionLatestCacheReset",
                json!({"distributionName": row.name}),
            )
            .await;
        assert_eq!(
            reset["createTime"],
            Value::Null,
            "case {case}: expected the new distribution to carry no cache reset, got {reset:?}"
        );
        // It is a NEW distribution, so the clock owes it its rollout: a reborn
        // name does not inherit the deployed status of the record that is gone.
        assert_eq!(
            reborn["status"].as_str(),
            Some(IN_PROGRESS),
            "case {case}: expected the new distribution to be {IN_PROGRESS}, got {reborn:?}"
        );
    }
}

/// RULE: the last reset of a distribution's cached content lives ON that
/// distribution's record, so deleting the distribution takes the reset with it
/// and the account-wide report is the latest reset of the distributions that are
/// still there.
#[tokio::test]
async fn a_deleted_distribution_takes_its_cache_reset_with_it() {
    let emulator = Emulator::new();
    create_origin(&emulator, OriginKind::Instance, "reset-origin").await;
    create_distribution(&emulator, "kept-cdn", "small_1_0", "reset-origin").await;
    create_distribution(&emulator, "gone-cdn", "medium_1_0", "reset-origin").await;
    for name in ["kept-cdn", "gone-cdn"] {
        emulator
            .ok("ResetDistributionCache", json!({"distributionName": name}))
            .await;
    }
    let kept = emulator
        .ok(
            "GetDistributionLatestCacheReset",
            json!({"distributionName": "kept-cdn"}),
        )
        .await;

    emulator
        .ok(DELETE, json!({"distributionName": "gone-cdn"}))
        .await;

    let case = "one of two reset distributions deleted";
    let account_wide = emulator
        .ok("GetDistributionLatestCacheReset", json!({}))
        .await;
    assert_eq!(
        account_wide["createTime"], kept["createTime"],
        "case {case}: expected the surviving distribution's reset, got {account_wide:?}"
    );
    assert!(
        account_wide["status"].is_string(),
        "case {case}: expected the surviving reset's status, got {account_wide:?}"
    );

    emulator
        .ok(DELETE, json!({"distributionName": "kept-cdn"}))
        .await;
    let case = "every reset distribution deleted";
    let account_wide = emulator
        .ok("GetDistributionLatestCacheReset", json!({}))
        .await;
    assert_eq!(
        account_wide["createTime"],
        Value::Null,
        "case {case}: expected no reset left to report, got {account_wide:?}"
    );
    assert_eq!(
        account_wide["status"],
        Value::Null,
        "case {case}: expected no reset status left to report, got {account_wide:?}"
    );
}

// ---------------------------------------------------------------------------
// The refusal table
// ---------------------------------------------------------------------------

/// One `DeleteDistribution` request the service must refuse, and why.
struct Refusal {
    case: &'static str,
    /// The name the request carries, or `None` for a request that names no
    /// distribution at all — the model leaves `distributionName` optional, and
    /// the operation documents no behaviour for an omitted one.
    distribution_name: Option<&'static str>,
    /// The wire error code the refusal carries.
    error_code: &'static str,
}

/// The table.
const REFUSALS: &[Refusal] = &[
    Refusal {
        case: "a name this account holds no distribution under",
        distribution_name: Some("ghost-cdn"),
        error_code: NOT_FOUND,
    },
    Refusal {
        case: "a name that belongs to a resource of another kind",
        distribution_name: Some("guard-origin"),
        error_code: NOT_FOUND,
    },
    Refusal {
        case: "a name that belongs to the certificate the distribution serves with",
        distribution_name: Some("guard-cert"),
        error_code: NOT_FOUND,
    },
    Refusal {
        case: "no distribution named at all",
        distribution_name: None,
        error_code: INVALID_INPUT,
    },
];

/// RULE: a delete that names no distribution of this account deletes NOTHING,
/// and says which rule the request broke rather than reporting a deletion.
#[tokio::test]
async fn a_refused_delete_deletes_nothing() {
    let emulator = Emulator::new();
    create_origin(&emulator, OriginKind::Instance, "guard-origin").await;
    create_certificate(&emulator, "guard-cert", "guard.example.com", &[]).await;
    create_distribution(&emulator, "guard-cdn", "small_1_0", "guard-origin").await;
    let guard_before = distribution(&emulator, "guard-cdn").await;

    for refusal in REFUSALS {
        let case = refusal.case;
        let request = match refusal.distribution_name {
            Some(name) => json!({"distributionName": name}),
            None => json!({}),
        };
        let (status, body) = emulator.call(DELETE, request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body:?}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, refusal.error_code,
            "case {case}: expected {}, got {code} {body:?}",
            refusal.error_code
        );
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "case {case}: expected a message saying what was refused, got {body:?}"
        );
        if refusal.error_code == NOT_FOUND {
            assert_eq!(
                body["code"].as_str(),
                Some(DOES_NOT_EXIST),
                "case {case}: expected the body code {DOES_NOT_EXIST}, got {body:?}"
            );
            let named = refusal.distribution_name.unwrap_or_default();
            assert_eq!(
                body["message"].as_str(),
                Some(does_not_exist_message(named).as_str()),
                "case {case}: expected the refusal to name the resource type and {named}, got \
                 {body:?}"
            );
        }
        let guard_after = distribution(&emulator, "guard-cdn").await;
        assert_eq!(
            guard_after, guard_before,
            "case {case}: expected the refusal to delete nothing, got {guard_after:?}"
        );
        // A name of a resource of another kind is not deleted by a distribution
        // delete either.
        let instance = emulator
            .ok("GetInstance", json!({"instanceName": "guard-origin"}))
            .await["instance"]
            .clone();
        assert_eq!(
            instance["name"].as_str(),
            Some("guard-origin"),
            "case {case}: expected the instance to be untouched, got {instance:?}"
        );
        let reported = certificate(&emulator, "guard-cert").await;
        assert_eq!(
            reported["certificateName"].as_str(),
            Some("guard-cert"),
            "case {case}: expected the certificate to be untouched, got {reported:?}"
        );
    }
}

/// Every Region a distribution call can be made in that is NOT the distribution
/// Region.
///
/// Taken from the crate's own `REGION_NAME_VALUES` — the modelled `RegionName`
/// enum's values (`smithy_query(query="shape",shape="RegionName")`) — minus the
/// distribution Region, so the table cannot drift from the model as the catalog
/// grows, plus [`OFF_ENUM_REGION`].
fn other_regions() -> Vec<&'static str> {
    assert!(
        REGION_NAME_VALUES.contains(&DISTRIBUTION_REGION),
        "case the Region catalog: expected the modelled RegionName enum to list the distribution \
         Region, got {REGION_NAME_VALUES:?}"
    );
    assert!(
        !REGION_NAME_VALUES.contains(&OFF_ENUM_REGION),
        "case the Region catalog: expected {OFF_ENUM_REGION} to sit OUTSIDE the modelled \
         RegionName enum, got {REGION_NAME_VALUES:?}"
    );
    let mut regions: Vec<&'static str> = REGION_NAME_VALUES
        .iter()
        .copied()
        .filter(|region| *region != DISTRIBUTION_REGION)
        .collect();
    regions.push(OFF_ENUM_REGION);
    regions
}

/// RULE: the distribution APIs are served by the N. Virginia (`us-east-1`)
/// endpoint alone, so a delete signed for any other Region is refused with the
/// documented message and deletes nothing — while the same delete signed for
/// that Region, or carrying no scope at all, is served.
#[tokio::test]
async fn a_delete_is_served_by_the_distribution_region_alone() {
    let emulator = Emulator::new();
    create_origin(&emulator, OriginKind::Instance, "region-origin").await;
    create_distribution(&emulator, "region-cdn", "small_1_0", "region-origin").await;
    create_distribution(&emulator, "scoped-cdn", "small_1_0", "region-origin").await;
    let before = distribution(&emulator, "region-cdn").await;

    for region in other_regions() {
        let case = format!("a delete signed for {region}");
        let (status, body) = emulator
            .call_in(Some(region), DELETE, json!({"distributionName": "region-cdn"}))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body:?}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, INVALID_INPUT,
            "case {case}: expected {INVALID_INPUT}, got {code} {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(REGION_REFUSAL),
            "case {case}: expected the documented refusal, got {body:?}"
        );
        let after = distribution(&emulator, "region-cdn").await;
        assert_eq!(
            after, before,
            "case {case}: expected the refusal to delete nothing, got {after:?}"
        );
    }

    // Signed for the distribution Region, and with no credential scope at all:
    // both are served.
    emulator
        .ok_at(
            None,
            Some(DISTRIBUTION_REGION),
            DELETE,
            json!({"distributionName": "scoped-cdn"}),
        )
        .await;
    assert_does_not_exist(&emulator, "deleted in the distribution Region", "scoped-cdn").await;
    emulator
        .ok(DELETE, json!({"distributionName": "region-cdn"}))
        .await;
    assert_does_not_exist(&emulator, "deleted with no credential scope", "region-cdn").await;
}
