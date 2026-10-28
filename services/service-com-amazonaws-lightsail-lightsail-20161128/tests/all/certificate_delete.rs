// SPDX-License-Identifier: CC0-1.0

//! Deleting a Lightsail SSL/TLS certificate, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The harness is the one `instance_lifecycle.rs` already has.
//!
//! The rule under test: `DeleteCertificate` removes the ONE stored certificate
//! its name addresses together with everything that lives on that record — the
//! domains it covered, their validation records, its tags, its serial number and
//! its validity window — so no later read, listing or active-name walk reaches
//! it, its name is free for a certificate that inherits nothing from it, and the
//! call answers exactly one TERMINAL `DeleteCertificate` operation-log entry
//! located where the certificate was. A certificate another resource is still
//! serving is in use — "Certificates that are currently attached to a
//! distribution cannot be deleted. Use the DetachCertificateFromDistribution
//! action to detach a certificate from a distribution." — so the delete is
//! refused with the modelled `InvalidInputException` and removes nothing, and a
//! name this account holds no certificate under is the service's `DoesNotExist`
//! rejection rather than a reported deletion that did not happen. The call is
//! served by the N. Virginia (`us-east-1`) endpoint ALONE, as the modelled
//! `InvalidInputException` documentation states for the domain and distribution
//! APIs together.
//!
//! So the cases are TABLES: one row per certificate shape (how many alternate
//! domains it covers, how many tags it carries, whether the certificate
//! authority has issued it when the delete lands, whether another certificate
//! stands beside it); the in-use refusal is a table over how many distributions
//! serve the certificate and how each one took it; the not-found refusal is a
//! table over the kinds of name that address no certificate; and the Region
//! restriction is a table over Regions the service serves resources in plus one
//! it does not.

use std::collections::BTreeSet;

use serde_json::{json, Value};

// The harness, the wire error code, the minted-id check, the instance stand-up
// and the certificate and distribution readers this test target already exposes:
// a sibling suite drives the service through the same front door rather than
// standing up a second one.
use crate::disk_attachment::create_running_instance;
use crate::distribution_certificate_attachment::{
    certificate, create_certificate, distribution, wait_for_issued_certificate,
};
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The operation under test, by the name its `X-Amz-Target` carries.
const DELETE: &str = "DeleteCertificate";

/// The `operationType` the log entry of a certificate's removal carries, the
/// modelled `OperationType` enum's own `DeleteCertificate` member
/// (`smithy_query(query="shape",shape="OperationType")`).
const OPERATION_TYPE: &str = "DeleteCertificate";

/// The `resourceType` the entry names, the modelled `ResourceType` enum's own
/// `Certificate` member.
const RESOURCE_TYPE: &str = "Certificate";

/// The terminal `Operation$status` of work that was already done when the call
/// returned, from the modelled `OperationStatus` enum.
const SUCCEEDED: &str = "Succeeded";

/// The one Region a Lightsail certificate lives in and is served from, from the
/// modelled `CreateCertificate` documentation ("Only certificates created in the
/// us-east-1 Amazon Web Services Region can be attached to Lightsail
/// distributions").
const CERTIFICATE_REGION: &str = "us-east-1";

/// The Availability Zone the origin instance of a distribution is created in:
/// the first zone of the certificate Region.
const ORIGIN_ZONE: &str = "us-east-1a";

/// The `CertificateStatus` a freshly requested certificate is in, from the
/// modelled enum.
const PENDING_VALIDATION: &str = "PENDING_VALIDATION";

/// The wire error code (awsJson `__type`) of Lightsail's rejection of a name
/// that addresses nothing, and the `code` its body carries — which a Lightsail
/// SDK client branches on rather than on the HTTP status.
const NOT_FOUND: &str = "NotFoundException";
/// See [`NOT_FOUND`].
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// The wire error code of a refusal of a request the model admits but the
/// service does not serve.
const INVALID_INPUT: &str = "InvalidInputException";

/// The action the documented refusal of an in-use certificate points the caller
/// at: `DeleteCertificate`'s own documentation reads "Certificates that are
/// currently attached to a distribution cannot be deleted. Use the
/// DetachCertificateFromDistribution action to detach a certificate from a
/// distribution."
const DETACH_ACTION: &str = "DetachCertificateFromDistribution";

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
/// not serve at all is no more the certificate Region than any other.
const OFF_ENUM_REGION: &str = "il-central-1";

/// An upper bound on the pages one listing walk may take, so a broken paging
/// contract fails the assertion rather than looping forever.
const MAX_PAGES: usize = 20;

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// Request the certificate `name` for `domain`, covering `alternatives` and
/// carrying `tags`, answering the `certificate` the create reports.
///
/// The sibling suite's own helper takes no tags, and a certificate's tags are
/// part of what the delete has to take with the record, so the body is built
/// here.
async fn create_tagged_certificate(
    emulator: &Emulator,
    name: &str,
    domain: &str,
    alternatives: &[&str],
    tags: &[(&str, &str)],
) -> Value {
    let mut body = json!({"certificateName": name, "domainName": domain});
    let map = body.as_object_mut().expect("the body is a JSON object");
    if !alternatives.is_empty() {
        map.insert("subjectAlternativeNames".to_string(), json!(alternatives));
    }
    if !tags.is_empty() {
        map.insert(
            "tags".to_string(),
            Value::Array(
                tags.iter()
                    .map(|(key, value)| json!({"key": key, "value": value}))
                    .collect(),
            ),
        );
    }
    emulator.ok("CreateCertificate", body).await["certificate"].clone()
}

/// Every certificate name `GetCertificates` reports, walking every page.
async fn listed_names(emulator: &Emulator) -> Vec<String> {
    let mut names = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let request = match &token {
            Some(token) => json!({"pageToken": token}),
            None => json!({}),
        };
        let page = emulator.ok("GetCertificates", request).await;
        for reported in page["certificates"].as_array().unwrap_or_else(|| {
            panic!("case listing certificates: expected a certificates array, got {page:?}")
        }) {
            names.push(
                reported["certificateName"]
                    .as_str()
                    .unwrap_or("<no name>")
                    .to_string(),
            );
        }
        match page["nextPageToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return names,
        }
    }
    panic!("case listing certificates: the paged walk did not terminate within {MAX_PAGES} pages");
}

/// Every name `GetActiveNames` reports for the certificate Region, walking every
/// page: a certificate's name is one of the names the account's resources hold.
async fn active_names(emulator: &Emulator) -> Vec<String> {
    let mut names = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let request = match &token {
            Some(token) => json!({"pageToken": token}),
            None => json!({}),
        };
        let page = emulator
            .ok_at(None, Some(CERTIFICATE_REGION), "GetActiveNames", request)
            .await;
        for reported in page["activeNames"].as_array().unwrap_or_else(|| {
            panic!("case listing active names: expected an activeNames array, got {page:?}")
        }) {
            names.push(reported.as_str().unwrap_or("<not a string>").to_string());
        }
        match page["nextPageToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return names,
        }
    }
    panic!("case listing active names: the paged walk did not terminate within {MAX_PAGES} pages");
}

/// Every operation-log entry `GetOperationsForResource` reports for `name`,
/// walking every page.
async fn operations_for(emulator: &Emulator, name: &str) -> Vec<Value> {
    let mut entries = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let request = match &token {
            Some(token) => json!({"resourceName": name, "pageToken": token}),
            None => json!({"resourceName": name}),
        };
        let page = emulator.ok("GetOperationsForResource", request).await;
        for entry in page["operations"].as_array().unwrap_or_else(|| {
            panic!("case reading the log of {name}: expected an operations array, got {page:?}")
        }) {
            entries.push(entry.clone());
        }
        match page["nextPageToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return entries,
        }
    }
    panic!("case reading the log of {name}: the walk did not terminate within {MAX_PAGES} pages");
}

/// The message Lightsail's `DoesNotExist` envelope carries for a name this
/// account holds no certificate under: the resource's TYPE and the identifier the
/// request carried (`crate::scaffolded::Scaffolded::resource_does_not_exist`).
fn does_not_exist_message(name: &str) -> String {
    format!("The {RESOURCE_TYPE} does not exist: {name}")
}

/// Require that `name` names no certificate of this account: every reader and
/// writer of a certificate answers HTTP 400, the wire error `NotFoundException`
/// and `DoesNotExist` in the body, with a message naming the resource type and
/// the requested name.
async fn assert_does_not_exist(emulator: &Emulator, case: &str, name: &str) {
    for operation in ["GetCertificates", DELETE] {
        let (status, body) = emulator
            .call(operation, json!({"certificateName": name}))
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

/// Require that the one entry `answered` carries is the terminal
/// `DeleteCertificate` log entry for `name`, answering its id.
fn assert_delete_entry(case: &str, answered: &Value, name: &str) -> String {
    let entries = answered["operations"].as_array().unwrap_or_else(|| {
        panic!("case {case}: expected an operations array from {DELETE}, got {answered:?}")
    });
    assert_eq!(
        entries.len(),
        1,
        "case {case}: expected exactly one log entry for the one certificate deleted, got \
         {answered:?}"
    );
    let entry = &entries[0];
    assert_eq!(
        entry["operationType"].as_str(),
        Some(OPERATION_TYPE),
        "case {case}: expected the {OPERATION_TYPE} operation type, got {entry:?}"
    );
    assert_eq!(
        entry["resourceType"].as_str(),
        Some(RESOURCE_TYPE),
        "case {case}: expected the entry to be about the certificate, got {entry:?}"
    );
    assert_eq!(
        entry["resourceName"].as_str(),
        Some(name),
        "case {case}: expected the entry to name {name}, got {entry:?}"
    );
    assert_eq!(
        entry["status"].as_str(),
        Some(SUCCEEDED),
        "case {case}: expected a {SUCCEEDED} entry for work that was done when the call \
         returned, got {entry:?}"
    );
    assert_eq!(
        entry["isTerminal"].as_bool(),
        Some(true),
        "case {case}: expected a terminal entry — the certificate is gone, so nothing is left \
         for a status to settle on — got {entry:?}"
    );
    assert_eq!(
        entry["location"]["regionName"].as_str(),
        Some(CERTIFICATE_REGION),
        "case {case}: expected the entry located where the certificate was, got {entry:?}"
    );
    assert!(
        entry["createdAt"].as_f64().is_some(),
        "case {case}: expected the entry to carry the instant it was made, got {entry:?}"
    );
    let id = entry["id"].as_str().unwrap_or_default().to_string();
    assert!(
        is_uuid_shaped(&id),
        "case {case}: expected a uuid-shaped entry id, got {id:?}"
    );
    id
}

/// The `(key, value)` tags the JSON tag list `value` carries, sorted.
fn tag_pairs(value: &Value) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = value
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|tag| {
                    (
                        tag["key"].as_str().unwrap_or("<no key>").to_string(),
                        tag["value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    pairs.sort();
    pairs
}

// ---------------------------------------------------------------------------
// The delete table
// ---------------------------------------------------------------------------

/// One certificate to request and then delete.
///
/// The rows differ along everything the rule generalises over: how many
/// alternate domains the certificate covers (none, one, and the documented
/// maximum of nine), how many tags it carries, the form of its name, whether the
/// certificate authority has issued it when the delete lands, and whether
/// another certificate of the account stands beside it.
struct Row {
    case: &'static str,
    name: &'static str,
    domain: &'static str,
    /// The alternate domains and subdomains the certificate also covers. The
    /// documented bound is nine, from the modelled
    /// `CreateCertificate$subjectAlternativeNames` documentation ("You can
    /// specify a maximum of nine alternate domains (in addition to the primary
    /// domain name)").
    alternatives: &'static [&'static str],
    tags: &'static [(&'static str, &'static str)],
    /// Whether the certificate is waited into `ISSUED` before the delete: the
    /// documented rule makes the delete conditional on the certificate being
    /// attached to NOTHING, not on the status the authority gave it.
    wait_issued: bool,
    /// Another certificate of the account, which the delete must leave exactly
    /// as it is.
    neighbour: Option<&'static str>,
}

/// The table.
const ROWS: &[Row] = &[
    Row {
        case: "one domain, no tag, deleted while the authority has not issued it yet",
        name: "bare-cert",
        domain: "bare.example.com",
        alternatives: &[],
        tags: &[],
        wait_issued: false,
        neighbour: None,
    },
    Row {
        case: "one alternate domain, one tag, deleted once issued, a neighbour beside it",
        name: "issued-cert",
        domain: "shop.example.net",
        alternatives: &["www.shop.example.net"],
        tags: &[("env", "test")],
        wait_issued: true,
        neighbour: Some("issued-keep-cert"),
    },
    Row {
        case: "the documented maximum of nine alternate domains and three tags",
        name: "wide-cert",
        domain: "alpha.example.org",
        alternatives: &[
            "beta.example.org",
            "gamma.example.org",
            "delta.example.org",
            "epsilon.example.org",
            "zeta.example.org",
            "eta.example.org",
            "theta.example.org",
            "iota.example.org",
            "kappa.example.org",
        ],
        tags: &[("env", "prod"), ("team", "payments"), ("tier", "edge")],
        wait_issued: true,
        neighbour: Some("wide-keep-cert"),
    },
    Row {
        case: "a name carrying an underscore, deleted while still pending validation",
        name: "under_score_cert",
        domain: "under.example.com",
        alternatives: &["cdn.under.example.com", "img.under.example.com"],
        tags: &[("owner", "team-a")],
        wait_issued: false,
        neighbour: Some("under-keep-cert"),
    },
];

/// RULE: `DeleteCertificate` removes the stored certificate its name addresses
/// with everything that lives on that record, answering one terminal
/// `DeleteCertificate` log entry located where the certificate was; the record
/// is then gone from every read, listing and active-name walk, its name is free
/// for a certificate that inherits nothing from it, and every certificate beside
/// it survives.
#[tokio::test]
async fn deleting_a_certificate_removes_it_and_everything_it_held() {
    let emulator = Emulator::new();
    let mut entry_ids: BTreeSet<String> = BTreeSet::new();

    for row in ROWS {
        let case = row.case;
        if let Some(neighbour) = row.neighbour {
            create_tagged_certificate(
                &emulator,
                neighbour,
                "neighbour.example.com",
                &[],
                &[("role", "neighbour")],
            )
            .await;
        }
        let created = create_tagged_certificate(
            &emulator,
            row.name,
            row.domain,
            row.alternatives,
            row.tags,
        )
        .await;
        // The create's own answer, so no clock has run yet.
        assert_eq!(
            created["certificateDetail"]["status"].as_str(),
            Some(PENDING_VALIDATION),
            "case {case}: expected a freshly requested certificate to be {PENDING_VALIDATION}, \
             got {created:?}"
        );
        let created_arn = created["certificateArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !created_arn.is_empty(),
            "case {case}: expected the created certificate to carry an ARN, got {created:?}"
        );
        if row.wait_issued {
            wait_for_issued_certificate(&emulator, row.name).await;
        }

        // The record the delete is about to remove, as a read reports it: the
        // domains it covers and the tags it carries all live on it.
        let stored = certificate(&emulator, row.name).await;
        assert_eq!(
            stored["certificateDetail"]["domainName"].as_str(),
            Some(row.domain),
            "case {case}: expected the stored certificate to be for {}, got {stored:?}",
            row.domain
        );
        assert_eq!(
            stored["certificateDetail"]["inUseResourceCount"].as_i64(),
            Some(0),
            "case {case}: expected a certificate attached to nothing to be in use by 0 \
             resources, got {stored:?}"
        );
        let mut expected_tags: Vec<(String, String)> = row
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        expected_tags.sort();
        assert_eq!(
            tag_pairs(&stored["certificateDetail"]["tags"]),
            expected_tags,
            "case {case}: expected the tags the create asked for, got {stored:?}"
        );

        let answered = emulator
            .ok(DELETE, json!({"certificateName": row.name}))
            .await;
        let id = assert_delete_entry(case, &answered, row.name);
        assert!(
            entry_ids.insert(id.clone()),
            "case {case}: expected a fresh log entry id, got the already seen {id}"
        );

        // The record is gone: a named read of it is the DoesNotExist rejection,
        // and so is a second delete of the same certificate.
        assert_does_not_exist(&emulator, case, row.name).await;
        let listed = listed_names(&emulator).await;
        assert!(
            !listed.contains(&row.name.to_string()),
            "case {case}: expected the deleted certificate to be gone from the listing, got \
             {listed:?}"
        );
        if let Some(neighbour) = row.neighbour {
            assert!(
                listed.contains(&neighbour.to_string()),
                "case {case}: expected the certificate beside it to survive, got {listed:?}"
            );
            let kept = certificate(&emulator, neighbour).await;
            assert_eq!(
                kept["certificateDetail"]["domainName"].as_str(),
                Some("neighbour.example.com"),
                "case {case}: expected the neighbour untouched, got {kept:?}"
            );
        }
        let active = active_names(&emulator).await;
        assert!(
            !active.contains(&row.name.to_string()),
            "case {case}: expected the deleted certificate's name to be gone from the active \
             names, got {active:?}"
        );

        // The log is append-only, so the entry this delete wrote is readable
        // back by the resource it was about even though that resource is gone.
        let logged = operations_for(&emulator, row.name).await;
        assert!(
            logged
                .iter()
                .any(|entry| entry["id"].as_str() == Some(id.as_str())
                    && entry["operationType"].as_str() == Some(OPERATION_TYPE)),
            "case {case}: expected the {OPERATION_TYPE} entry {id} in the log of {}, got \
             {logged:?}",
            row.name
        );

        // The name is free again, and a certificate requested under it inherits
        // NOTHING from the deleted one: its own domain, no tag, a fresh ARN and
        // the pending status every new certificate starts in.
        let reborn = create_tagged_certificate(
            &emulator,
            row.name,
            "reborn.example.com",
            &[],
            &[],
        )
        .await;
        assert_eq!(
            reborn["certificateDetail"]["domainName"].as_str(),
            Some("reborn.example.com"),
            "case {case}: expected the new certificate's own domain, got {reborn:?}"
        );
        assert!(
            tag_pairs(&reborn["certificateDetail"]["tags"]).is_empty(),
            "case {case}: expected the new certificate to carry no tag of the deleted one, got \
             {reborn:?}"
        );
        assert_eq!(
            reborn["certificateDetail"]["status"].as_str(),
            Some(PENDING_VALIDATION),
            "case {case}: expected the new certificate to start {PENDING_VALIDATION}, got \
             {reborn:?}"
        );
        assert_ne!(
            reborn["certificateArn"].as_str(),
            Some(created_arn.as_str()),
            "case {case}: expected a certificate of its own rather than the deleted one's ARN, \
             got {reborn:?}"
        );
        assert!(
            string_list(&reborn["certificateDetail"]["subjectAlternativeNames"]).is_empty(),
            "case {case}: expected none of the deleted certificate's alternate domains, got \
             {reborn:?}"
        );
    }
}

/// The strings the JSON list `value` carries.
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
// The in-use refusal
// ---------------------------------------------------------------------------

/// One certificate and the distributions that serve it.
///
/// The rows differ along what the rule generalises over: HOW a distribution took
/// the certificate (named at `CreateDistribution` time, or recorded by a later
/// `AttachCertificateToDistribution`) and HOW MANY distributions serve it, so
/// the refusal is asserted at one attachment and at two, and the delete is only
/// served once the last of them is gone.
struct InUseRow {
    case: &'static str,
    certificate_name: &'static str,
    domain: &'static str,
    /// `(distributionName, named_at_create)` per distribution serving the
    /// certificate.
    distributions: &'static [(&'static str, bool)],
}

/// The table.
const IN_USE_ROWS: &[InUseRow] = &[
    InUseRow {
        case: "one distribution that named the certificate at create time",
        certificate_name: "in-use-create-cert",
        domain: "create.example.com",
        distributions: &[("in-use-create-cdn", true)],
    },
    InUseRow {
        case: "one distribution the certificate was attached to afterwards",
        certificate_name: "in-use-attach-cert",
        domain: "attach.example.com",
        distributions: &[("in-use-attach-cdn", false)],
    },
    InUseRow {
        case: "two distributions, one of each kind of attachment",
        certificate_name: "in-use-pair-cert",
        domain: "pair.example.com",
        distributions: &[("in-use-pair-one-cdn", true), ("in-use-pair-two-cdn", false)],
    },
];

/// Require that `DeleteCertificate` refuses `name` as in use by `in_use`
/// resources and removes nothing.
///
/// `pub(crate)` so a sibling suite in this test target asserts the one in-use
/// refusal through this one helper rather than carrying a second copy of it.
pub(crate) async fn assert_in_use_refusal(
    emulator: &Emulator,
    case: &str,
    name: &str,
    in_use: i64,
) {
    let (status, body) = emulator.call(DELETE, json!({"certificateName": name})).await;
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400 for a certificate in use by {in_use}, got {status} \
         {body:?}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, INVALID_INPUT,
        "case {case}: expected {INVALID_INPUT} for a certificate in use, got {code} {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(name) && message.contains(&in_use.to_string()),
        "case {case}: expected the refusal to name {name} and the {in_use} resources serving \
         it, got {body:?}"
    );
    assert!(
        message.contains(DETACH_ACTION),
        "case {case}: expected the refusal to point at {DETACH_ACTION}, got {body:?}"
    );
    // Refused, so nothing was removed: the certificate is still there, still
    // counting every resource that serves it.
    let survived = certificate(emulator, name).await;
    assert_eq!(
        survived["certificateDetail"]["inUseResourceCount"].as_i64(),
        Some(in_use),
        "case {case}: expected the refused certificate still in use by {in_use}, got \
         {survived:?}"
    );
}

/// RULE: a certificate another resource is still serving is in use, so the
/// delete is refused with the modelled `InvalidInputException` and removes
/// nothing; it is served only once nothing names the certificate any more.
#[tokio::test]
async fn deleting_a_certificate_a_distribution_serves_is_refused() {
    let emulator = Emulator::new();
    create_running_instance(&emulator, "in-use-origin", ORIGIN_ZONE).await;

    for row in IN_USE_ROWS {
        let case = row.case;
        create_certificate(&emulator, row.certificate_name, row.domain, &[]).await;
        wait_for_issued_certificate(&emulator, row.certificate_name).await;

        for (distribution_name, named_at_create) in row.distributions {
            let mut body = json!({
                "distributionName": distribution_name,
                "bundleId": "small_1_0",
                "origin": {"name": "in-use-origin"},
                "defaultCacheBehavior": {"behavior": "cache"},
            });
            if *named_at_create {
                body["certificateName"] = json!(row.certificate_name);
            }
            emulator.ok("CreateDistribution", body).await;
            if !named_at_create {
                emulator
                    .ok(
                        "AttachCertificateToDistribution",
                        json!({
                            "distributionName": distribution_name,
                            "certificateName": row.certificate_name,
                        }),
                    )
                    .await;
            }
            let serving = distribution(&emulator, distribution_name).await;
            assert_eq!(
                serving["certificateName"].as_str(),
                Some(row.certificate_name),
                "case {case}: expected {distribution_name} to serve {}, got {serving:?}",
                row.certificate_name
            );
        }

        // Every distribution still serving the certificate keeps the delete
        // refused, and the count the refusal reports falls as they go.
        let mut remaining =
            i64::try_from(row.distributions.len()).expect("a table row names a few distributions");
        assert_in_use_refusal(&emulator, case, row.certificate_name, remaining).await;
        for (distribution_name, _) in row.distributions {
            emulator
                .ok(
                    "DeleteDistribution",
                    json!({"distributionName": distribution_name}),
                )
                .await;
            remaining -= 1;
            if remaining > 0 {
                assert_in_use_refusal(&emulator, case, row.certificate_name, remaining).await;
            }
        }

        // Nothing names the certificate any more, so the same delete is served.
        let answered = emulator
            .ok(DELETE, json!({"certificateName": row.certificate_name}))
            .await;
        assert_delete_entry(case, &answered, row.certificate_name);
        assert_does_not_exist(&emulator, case, row.certificate_name).await;
    }
}

// ---------------------------------------------------------------------------
// The not-found refusal
// ---------------------------------------------------------------------------

/// RULE: a well-formed name this account holds no certificate under is the
/// service's `DoesNotExist` rejection rather than a reported deletion that did
/// not happen — including a name another kind of resource holds, which is no
/// certificate of this account.
#[tokio::test]
async fn deleting_a_name_that_names_no_certificate_is_refused() {
    let emulator = Emulator::new();
    create_running_instance(&emulator, "stranger-instance", ORIGIN_ZONE).await;
    create_certificate(&emulator, "stranger-cert", "stranger.example.com", &[]).await;
    emulator
        .ok(DELETE, json!({"certificateName": "stranger-cert"}))
        .await;

    // Rows spanning the kinds of name that address no certificate: one nothing
    // of the account ever held, one a certificate held until a moment ago, one
    // that differs from a held name only in case, one held by a resource of
    // another kind, and one that is whitespace alone.
    for name in [
        "never-held-cert",
        "stranger-cert",
        "STRANGER-CERT",
        "stranger-instance",
        "   ",
    ] {
        assert_does_not_exist(&emulator, &format!("the name {name:?}"), name).await;
    }
}

// ---------------------------------------------------------------------------
// The Region restriction
// ---------------------------------------------------------------------------

/// RULE: Lightsail serves the domain, distribution and certificate APIs from the
/// N. Virginia endpoint alone, so a delete signed for any other Region is
/// refused with the modelled `InvalidInputException` carrying the documented
/// message, and removes nothing.
#[tokio::test]
async fn a_delete_outside_the_certificate_region_is_refused() {
    let emulator = Emulator::new();
    create_certificate(&emulator, "region-bound-cert", "region.example.com", &[]).await;

    for region in ["us-west-2", "eu-central-1", OFF_ENUM_REGION] {
        let (status, body) = emulator
            .call_in(
                Some(region),
                DELETE,
                json!({"certificateName": "region-bound-cert"}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case signed for {region}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_INPUT,
            "case signed for {region}: expected {INVALID_INPUT}, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(REGION_REFUSAL),
            "case signed for {region}: expected the documented Region refusal, got {body:?}"
        );
        let survived = certificate(&emulator, "region-bound-cert").await;
        assert_eq!(
            survived["certificateName"].as_str(),
            Some("region-bound-cert"),
            "case signed for {region}: expected the refusal to delete nothing, got {survived:?}"
        );
    }

    // And the same call in the certificate Region is served.
    let answered = emulator
        .ok_at(
            None,
            Some(CERTIFICATE_REGION),
            DELETE,
            json!({"certificateName": "region-bound-cert"}),
        )
        .await;
    assert_delete_entry(
        &format!("signed for {CERTIFICATE_REGION}"),
        &answered,
        "region-bound-cert",
    );
}
