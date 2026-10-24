// SPDX-License-Identifier: CC0-1.0

//! Deleting a Lightsail load balancer, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `DeleteLoadBalancer` removes the ONE stored load balancer
//! of (Region, name) together with everything that lives on it — "Deletes a
//! Lightsail load balancer and all its associated SSL/TLS certificates. Once the
//! load balancer is deleted, you will need to create a new load balancer, create
//! a new certificate, and verify domain ownership again" — so no later read
//! reports it, its name and its share of the per-Region quota are free again, and
//! the call answers one terminal `DeleteLoadBalancer` operation-log entry located
//! in that Region. Nothing else is touched: another Region's namesake, another
//! load balancer of the same Region and every instance that was registered with
//! the deleted one all survive it. A name that names no load balancer in the
//! Region the request is made in — including one deleted a moment ago — is the
//! service's `DoesNotExist` rejection rather than a reported deletion that did
//! not happen.

use std::collections::BTreeSet;

use serde_json::{json, Value};

// The harness, the wire error code, the minted-id check and the instance
// stand-up this test target already exposes: a sibling suite drives the service
// through the same front door rather than standing up a second one.
use crate::disk_attachment::create_running_instance;
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The `operationType` the log entry of a load balancer's removal carries, the
/// modelled `OperationType` enum's own `DeleteLoadBalancer` member.
const OPERATION_TYPE: &str = "DeleteLoadBalancer";

/// The `resourceType` the entry names, the modelled `ResourceType` enum's own
/// `LoadBalancer` member.
const RESOURCE_TYPE: &str = "LoadBalancer";

/// The terminal `Operation$status` of work that was already done when the call
/// returned.
const SUCCEEDED: &str = "Succeeded";

/// The value Lightsail puts in a not-found body's `code` member, which a
/// Lightsail SDK client branches on rather than on the HTTP status.
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// The health check path a load balancer reports when the request named none,
/// from the modelled `CreateLoadBalancerRequest$healthCheckPath` documentation
/// ("Lightsail uses the root path of your website (\"/\")").
const DEFAULT_HEALTH_CHECK_PATH: &str = "/";

/// The documented per-Region cap (`CreateLoadBalancer`: "You can create up to 10
/// load balancers per AWS Region in your account").
const MAX_LOAD_BALANCERS_PER_REGION: usize = 10;

/// An upper bound on the pages one listing walk may take, so a broken paging
/// contract fails the assertion rather than looping forever.
const MAX_PAGES: usize = 20;

/// The stored load balancer `name` in `region`, as `GetLoadBalancer` reports it.
async fn load_balancer(emulator: &Emulator, region: &str, name: &str) -> Value {
    emulator
        .ok_at(
            None,
            Some(region),
            "GetLoadBalancer",
            json!({"loadBalancerName": name}),
        )
        .await["loadBalancer"]
        .clone()
}

/// Every load balancer name `GetLoadBalancers` reports in `region`, walking every
/// page.
async fn listed_names(emulator: &Emulator, region: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let body = match &token {
            Some(token) => json!({"pageToken": token}),
            None => json!({}),
        };
        let page = emulator
            .ok_at(None, Some(region), "GetLoadBalancers", body)
            .await;
        for balancer in page["loadBalancers"].as_array().unwrap_or_else(|| {
            panic!("case listing {region}: expected a loadBalancers array, got {page:?}")
        }) {
            names.push(
                balancer["name"]
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
    panic!("case listing {region}: the paged walk did not terminate within {MAX_PAGES} pages");
}

/// Require that `name` names no load balancer of `region`: HTTP 400, the wire
/// error `NotFoundException` and `DoesNotExist` in the body.
async fn assert_does_not_exist(emulator: &Emulator, case: &str, region: &str, name: &str) {
    let (status, body) = emulator
        .call_in(
            Some(region),
            "GetLoadBalancer",
            json!({"loadBalancerName": name}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400 reading {name} in {region}, got {status} {body:?}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, "NotFoundException",
        "case {case}: expected NotFoundException reading {name} in {region}, got {code} {body:?}"
    );
    assert_eq!(
        body["code"].as_str(),
        Some(DOES_NOT_EXIST),
        "case {case}: expected the body code {DOES_NOT_EXIST} reading {name}, got {body:?}"
    );
}

// ---------------------------------------------------------------------------
// The delete table
// ---------------------------------------------------------------------------

/// One load balancer to create and then delete.
///
/// The rows differ along everything the rule generalises over: the Region the
/// load balancer lives in, the instance port it was created on (including each
/// bound of the modelled -1..65535 range), how many tags it carries, whether it
/// holds an SSL/TLS certificate (and how many alternative names that carries),
/// how many instances are registered with it, whether it has finished
/// provisioning when the delete is made, and whether another load balancer of
/// the same Region stands beside it.
struct Row {
    case: &'static str,
    region: &'static str,
    name: &'static str,
    instance_port: i64,
    tags: &'static [(&'static str, &'static str)],
    /// `(certificateName, certificateDomainName, certificateAlternativeNames)`,
    /// or `None` for a load balancer created with no certificate.
    certificate: Option<(&'static str, &'static str, &'static [&'static str])>,
    /// The instances registered with the load balancer before the delete. Each
    /// is stood up running in this Region's first Availability Zone.
    instances: &'static [&'static str],
    /// Whether the load balancer is waited into `active` before the delete: a
    /// delete does not wait for the provisioning the clock owes a new record.
    wait_active: bool,
    /// Another load balancer of the same Region, which the delete must leave
    /// exactly as it is.
    neighbour: Option<&'static str>,
}

/// The table.
const ROWS: &[Row] = &[
    Row {
        case: "nothing optional, deleted while it is still provisioning",
        region: "us-east-1",
        name: "bare-lb",
        instance_port: 80,
        tags: &[],
        certificate: None,
        instances: &[],
        wait_active: false,
        neighbour: None,
    },
    Row {
        case: "active, one tag, one registered instance, a neighbour beside it",
        region: "us-west-2",
        name: "served-lb",
        instance_port: 8080,
        tags: &[("env", "test")],
        certificate: None,
        instances: &["del-web-1"],
        wait_active: true,
        neighbour: Some("served-keep-lb"),
    },
    Row {
        case: "a requested certificate goes with the load balancer",
        region: "eu-west-1",
        name: "cert-lb",
        instance_port: 443,
        tags: &[],
        certificate: Some(("del-site-cert", "example.com", &[])),
        instances: &[],
        wait_active: true,
        neighbour: None,
    },
    Row {
        case: "a certificate with two alternative names, three tags, two registered instances",
        region: "ap-southeast-2",
        name: "full-lb",
        instance_port: 3000,
        tags: &[("env", "prod"), ("team", "payments"), ("tier", "edge")],
        certificate: Some((
            "del-multi-cert",
            "example.com",
            &["www.example.com", "blog.example.com"],
        )),
        instances: &["del-app-1", "del-app-2"],
        wait_active: true,
        neighbour: Some("full-keep-lb"),
    },
    Row {
        case: "at the lowest modelled instance port, deleted while provisioning",
        region: "us-east-2",
        name: "low-port-lb",
        instance_port: -1,
        tags: &[("owner", "team-a")],
        certificate: None,
        instances: &[],
        wait_active: false,
        neighbour: None,
    },
    Row {
        case: "at the highest modelled instance port",
        region: "eu-central-1",
        name: "high-port-lb",
        instance_port: 65535,
        tags: &[],
        certificate: None,
        instances: &["del-early-1"],
        wait_active: true,
        neighbour: None,
    },
];

impl Row {
    /// This row as the `CreateLoadBalancer` request body.
    fn create_request(&self) -> Value {
        let mut body = json!({
            "loadBalancerName": self.name,
            "instancePort": self.instance_port,
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
        if let Some((name, domain, alternatives)) = self.certificate {
            map.insert("certificateName".to_string(), json!(name));
            map.insert("certificateDomainName".to_string(), json!(domain));
            if !alternatives.is_empty() {
                map.insert(
                    "certificateAlternativeNames".to_string(),
                    json!(alternatives),
                );
            }
        }
        body
    }

    /// The Availability Zone this row's instances run in: the Region's first.
    fn zone(&self) -> String {
        format!("{}a", self.region)
    }
}

/// Create the load balancer `name` in `region` on `instance_port`.
async fn create_load_balancer(emulator: &Emulator, region: &str, name: &str, instance_port: i64) {
    emulator
        .ok_at(
            None,
            Some(region),
            "CreateLoadBalancer",
            json!({"loadBalancerName": name, "instancePort": instance_port}),
        )
        .await;
}

/// Poll `GetLoadBalancer` until `name` reports `want` in `region`.
///
/// The transition is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_for_state(emulator: &Emulator, region: &str, name: &str, want: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let balancer = load_balancer(emulator, region, name).await;
        if balancer["state"].as_str() == Some(want) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case waiting for '{want}' on {name}: expected it within 5s, got {:?}",
            balancer["state"]
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

/// RULE: `DeleteLoadBalancer` removes the stored load balancer of (Region, name)
/// with everything that lives on it, answering one terminal operation-log entry
/// located in that Region; the record is then gone from every read, its name is
/// free for a new load balancer that inherits nothing from it, and the resources
/// beside it are untouched.
#[tokio::test]
async fn deleting_a_load_balancer_removes_it_and_everything_it_held() {
    let emulator = Emulator::new();
    let mut operation_ids: BTreeSet<String> = BTreeSet::new();

    for row in ROWS {
        let case = row.case;
        for name in row.instances {
            create_running_instance(&emulator, name, &row.zone()).await;
        }
        if let Some(neighbour) = row.neighbour {
            create_load_balancer(&emulator, row.region, neighbour, 80).await;
        }
        emulator
            .ok_at(
                None,
                Some(row.region),
                "CreateLoadBalancer",
                row.create_request(),
            )
            .await;
        if !row.instances.is_empty() {
            emulator
                .ok_at(
                    None,
                    Some(row.region),
                    "AttachInstancesToLoadBalancer",
                    json!({"loadBalancerName": row.name, "instanceNames": row.instances}),
                )
                .await;
        }
        if row.wait_active {
            wait_for_state(&emulator, row.region, row.name, "active").await;
        }

        // What the record reports BEFORE the delete, so the assertions below
        // compare against the load balancer that actually existed.
        let before = load_balancer(&emulator, row.region, row.name).await;
        let before_arn = before["arn"].as_str().unwrap_or_default().to_string();
        let before_dns = before["dnsName"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            before["tlsCertificateSummaries"].as_array().map(Vec::len),
            Some(usize::from(row.certificate.is_some())),
            "case {case}: expected the created certificate to be held, got {before:?}"
        );
        assert_eq!(
            before["instanceHealthSummary"].as_array().map(Vec::len),
            Some(row.instances.len()),
            "case {case}: expected {} registered instances, got {before:?}",
            row.instances.len()
        );
        let neighbour_before = match row.neighbour {
            Some(neighbour) => Some(load_balancer(&emulator, row.region, neighbour).await),
            None => None,
        };

        let answered = emulator
            .ok_at(
                None,
                Some(row.region),
                "DeleteLoadBalancer",
                json!({"loadBalancerName": row.name}),
            )
            .await;

        // One entry per resource the call affected, and this call deletes
        // exactly one load balancer.
        let operations = answered["operations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {answered:?}"))
            .clone();
        assert_eq!(
            operations.len(),
            1,
            "case {case}: expected one operation entry, got {operations:?}"
        );
        let operation = &operations[0];
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
        // A load balancer is spread "across Availability Zones", so its entry is
        // located in the Region and names no zone.
        assert_eq!(
            operation["location"]["regionName"].as_str(),
            Some(row.region),
            "case {case}: expected the entry in {}, got {operation:?}",
            row.region
        );
        assert_eq!(
            operation["location"]["availabilityZone"].as_str(),
            None,
            "case {case}: expected no availabilityZone on a Region-wide resource, got \
             {operation:?}"
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
        // response: the log reports the same entry under the same id.
        let logged = emulator
            .ok_at(None, Some(row.region), "GetOperation", json!({"operationId": id}))
            .await["operation"]
            .clone();
        assert_eq!(
            logged, *operation,
            "case {case}: expected the log to hold the answered entry, got {logged:?}"
        );

        // The record is gone from every read of it.
        assert_does_not_exist(&emulator, case, row.region, row.name).await;
        let listed = listed_names(&emulator, row.region).await;
        assert!(
            !listed.contains(&row.name.to_string()),
            "case {case}: expected the listing of {} not to report {}, got {listed:?}",
            row.region,
            row.name
        );
        // A second delete fails rather than reporting a deletion that did not
        // happen.
        let (status, body) = emulator
            .call_in(
                Some(row.region),
                "DeleteLoadBalancer",
                json!({"loadBalancerName": row.name}),
            )
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

        // Every instance that was registered with it survives the load balancer:
        // a registration lives on the load balancer, not on the instance.
        for name in row.instances {
            let instance = emulator
                .ok("GetInstance", json!({"instanceName": name}))
                .await["instance"]
                .clone();
            assert_eq!(
                instance["state"]["name"].as_str(),
                Some("running"),
                "case {case}: expected {name} to survive the load balancer, got {instance:?}"
            );
        }
        // So does another load balancer of the same Region, unchanged.
        if let (Some(neighbour), Some(expected)) = (row.neighbour, neighbour_before) {
            let after = load_balancer(&emulator, row.region, neighbour).await;
            assert_eq!(
                after, expected,
                "case {case}: expected {neighbour} to be untouched, got {after:?}"
            );
        }

        // "Once the load balancer is deleted, you will need to create a new load
        // balancer, create a new certificate, and verify domain ownership
        // again": the name is free, and what the new record holds comes from the
        // new request alone — no certificate, no registration, no tag and no
        // server-assigned member of the record that is gone.
        create_load_balancer(&emulator, row.region, row.name, 80).await;
        let reborn = load_balancer(&emulator, row.region, row.name).await;
        assert_eq!(
            reborn["tlsCertificateSummaries"].as_array().map(Vec::len),
            Some(0),
            "case {case}: expected the new load balancer to hold no certificate, got {reborn:?}"
        );
        assert_eq!(
            reborn["instanceHealthSummary"].as_array().map(Vec::len),
            Some(0),
            "case {case}: expected the new load balancer to hold no registration, got {reborn:?}"
        );
        assert_eq!(
            reborn["tags"].as_array().map(Vec::len),
            Some(0),
            "case {case}: expected the new load balancer to carry no tag, got {reborn:?}"
        );
        assert_eq!(
            reborn["instancePort"].as_i64(),
            Some(80),
            "case {case}: expected the new request's instance port, got {reborn:?}"
        );
        assert_eq!(
            reborn["healthCheckPath"].as_str(),
            Some(DEFAULT_HEALTH_CHECK_PATH),
            "case {case}: expected the documented default health check path, got {reborn:?}"
        );
        assert_ne!(
            reborn["arn"].as_str(),
            Some(before_arn.as_str()),
            "case {case}: expected a freshly minted ARN, got {reborn:?}"
        );
        assert_ne!(
            reborn["dnsName"].as_str(),
            Some(before_dns.as_str()),
            "case {case}: expected a freshly minted dnsName, got {reborn:?}"
        );
    }
}

/// RULE: a load balancer is addressed within the Region the request is made in,
/// so deleting one leaves its namesake in every other Region exactly as it is.
#[tokio::test]
async fn a_delete_reaches_only_the_load_balancer_of_its_own_region() {
    let emulator = Emulator::new();
    let regions = ["us-east-1", "us-west-2", "eu-west-1"];
    for region in regions {
        create_load_balancer(&emulator, region, "twin-lb", 80).await;
    }
    // What each namesake reports before the delete, so "unchanged" is checked
    // against the record itself rather than against its name alone.
    let mut before = Vec::new();
    for region in regions {
        before.push(load_balancer(&emulator, region, "twin-lb").await);
    }

    emulator
        .ok_at(
            None,
            Some("us-east-1"),
            "DeleteLoadBalancer",
            json!({"loadBalancerName": "twin-lb"}),
        )
        .await;

    assert_does_not_exist(&emulator, "the Region deleted in", "us-east-1", "twin-lb").await;
    for (index, region) in regions.iter().enumerate().skip(1) {
        let after = load_balancer(&emulator, region, "twin-lb").await;
        assert_eq!(
            after, before[index],
            "case the namesake in {region}: expected it to be untouched, got {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The refusal table
// ---------------------------------------------------------------------------

/// One `DeleteLoadBalancer` request the service must refuse, and why.
struct Refusal {
    case: &'static str,
    /// The Region the call is made in.
    region: &'static str,
    load_balancer_name: &'static str,
}

/// The table. Every row is refused the same way: Lightsail answers a resource it
/// cannot find with HTTP 400 and `DoesNotExist` in the body.
const REFUSALS: &[Refusal] = &[
    Refusal {
        case: "a name this account holds no load balancer under",
        region: "us-east-1",
        load_balancer_name: "ghost-lb",
    },
    Refusal {
        case: "the load balancer exists, but in another Region",
        region: "eu-west-1",
        load_balancer_name: "guard-lb",
    },
    Refusal {
        case: "a name that belongs to a resource of another kind",
        region: "us-east-1",
        load_balancer_name: "guard-instance",
    },
];

/// RULE: a delete naming no load balancer of the Region the request is made in is
/// the service's `DoesNotExist` rejection and deletes NOTHING.
#[tokio::test]
async fn a_refused_delete_deletes_nothing() {
    let emulator = Emulator::new();
    create_running_instance(&emulator, "guard-instance", "us-east-1a").await;
    create_load_balancer(&emulator, "us-east-1", "guard-lb", 80).await;
    let guard_before = load_balancer(&emulator, "us-east-1", "guard-lb").await;

    for refusal in REFUSALS {
        let case = refusal.case;
        let (status, body) = emulator
            .call_in(
                Some(refusal.region),
                "DeleteLoadBalancer",
                json!({"loadBalancerName": refusal.load_balancer_name}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body:?}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "NotFoundException",
            "case {case}: expected NotFoundException, got {code} {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some(DOES_NOT_EXIST),
            "case {case}: expected the body code {DOES_NOT_EXIST}, got {body:?}"
        );
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "case {case}: expected a message saying what was refused, got {body:?}"
        );
        let guard_after = load_balancer(&emulator, "us-east-1", "guard-lb").await;
        assert_eq!(
            guard_after, guard_before,
            "case {case}: expected the refusal to delete nothing, got {guard_after:?}"
        );
        // The instance the second row names is a resource of another kind, and a
        // load balancer delete is not what removes one.
        let instance = emulator
            .ok("GetInstance", json!({"instanceName": "guard-instance"}))
            .await["instance"]
            .clone();
        assert_eq!(
            instance["name"].as_str(),
            Some("guard-instance"),
            "case {case}: expected the instance to be untouched, got {instance:?}"
        );
    }
}

/// RULE: a deleted load balancer no longer counts against the documented
/// per-Region quota, so a Region that was full admits a new load balancer for
/// each one deleted — and the quota is counted per Region, so filling one Region
/// blocks nothing in another.
#[tokio::test]
async fn a_delete_frees_the_per_region_quota() {
    let emulator = Emulator::new();
    let region = "ca-central-1";
    let names: Vec<String> = (0..MAX_LOAD_BALANCERS_PER_REGION)
        .map(|index| format!("quota-lb-{index}"))
        .collect();
    for name in &names {
        create_load_balancer(&emulator, region, name, 80).await;
    }

    let case = "one more than the per-Region cap";
    let (status, body) = emulator
        .call_in(
            Some(region),
            "CreateLoadBalancer",
            json!({"loadBalancerName": "quota-lb-overflow", "instancePort": 80}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} {body:?}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, "InvalidInputException",
        "case {case}: expected InvalidInputException, got {code} {body:?}"
    );
    // A full Region blocks nothing in another: the cap is per Region.
    create_load_balancer(&emulator, "us-east-1", "quota-lb-overflow", 80).await;

    // Each delete gives the Region room for exactly one more load balancer.
    for (index, deleted) in names.iter().enumerate().take(3) {
        let case = format!("after deleting {deleted}");
        emulator
            .ok_at(
                None,
                Some(region),
                "DeleteLoadBalancer",
                json!({"loadBalancerName": deleted}),
            )
            .await;
        let replacement = format!("quota-lb-replacement-{index}");
        create_load_balancer(&emulator, region, &replacement, 80).await;
        let listed = listed_names(&emulator, region).await;
        assert_eq!(
            listed.len(),
            MAX_LOAD_BALANCERS_PER_REGION,
            "case {case}: expected the Region to hold {MAX_LOAD_BALANCERS_PER_REGION} load \
             balancers, got {listed:?}"
        );
        assert!(
            !listed.contains(deleted) && listed.contains(&replacement),
            "case {case}: expected {replacement} in place of {deleted}, got {listed:?}"
        );
        // Still full: the room the delete made is taken by the replacement.
        let (status, body) = emulator
            .call_in(
                Some(region),
                "CreateLoadBalancer",
                json!({"loadBalancerName": "quota-lb-overflow", "instancePort": 80}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the refilled Region to be full again, got {status} {body:?}"
        );
    }
}
