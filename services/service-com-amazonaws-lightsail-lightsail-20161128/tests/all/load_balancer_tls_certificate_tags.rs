// SPDX-License-Identifier: CC0-1.0

//! Tagging and untagging the SSL/TLS certificates a Lightsail load balancer
//! holds — `TagResource` and `UntagResource` — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The harness and the tag helpers are the ones `instance_lifecycle.rs`
//! and `resource_tags.rs` already expose, so this suite adds rows to the same
//! class rather than standing up a second harness.
//!
//! The rule under test: a tag change reaches EVERY resource of the account whose
//! modelled shape carries a `tags` member, including one whose record is held on
//! the record of another resource — the modelled `LoadBalancerTlsCertificate`
//! carries `tags`, `arn` and `location` of its own, so a certificate of a load
//! balancer is resolved by the name its own certificate operations address it by,
//! in the Region the load balancer that holds it lives in, and the merge-by-key
//! and removal-by-key are written to THAT record alone: not to the load balancer,
//! not to another of its certificates, and not to a certificate of the same name
//! held in another Region. A `resourceArn` the request carries as well must be
//! that certificate's own, the resulting set is bounded by the 50 tags a
//! Lightsail resource may carry, and a name this account holds no resource of in
//! the Region of the call is refused with the service's `DoesNotExist` envelope.
//!
//! The rows differ along what the rule generalises over: the Region, which call
//! created the certificate (`CreateLoadBalancer`'s own certificate members, which
//! write no tags for it, or `CreateLoadBalancerTlsCertificate`, which writes the
//! tags its request named), whether it is the load balancer's first or second
//! certificate, the form of its name and domain, and the shape of the tag set its
//! create left (none, one, four including a key-only tag).
//!
//! `GetLoadBalancerTlsCertificates` is what every tag change here is read back
//! through, so each case observes the write exactly as a client does.

use serde_json::{json, Value};

use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};
// The rule, the request shapes and the log-entry assertion are the tag suites':
// this one adds the kind whose record is held on another resource's record.
use crate::resource_tags::{
    assert_entry, reported_tags, tag_list, untag, want_tags, TagRow, DEFAULT_REGION,
};

/// The `ResourceType` an SSL/TLS certificate of a load balancer reports, from the
/// modelled `ResourceType` enum — the value the modelled
/// `LoadBalancerTlsCertificate$resourceType` member carries.
const CERTIFICATE_RESOURCE_TYPE: &str = "LoadBalancerTlsCertificate";

/// A Region that is not the one an unsigned request is served in, so a row can
/// pin that a certificate is resolved in the Region of the load balancer that
/// holds it.
const OTHER_REGION: &str = "eu-central-1";

/// The most tags one resource may carry, from the modelled `TagResource`
/// documentation ("Each resource can have a maximum of 50 tags").
const MAX_TAGS: usize = 50;

/// The instance port every load balancer here directs traffic to — a required
/// member of `CreateLoadBalancer` that nothing in this suite is about.
const INSTANCE_PORT: i64 = 80;

/// The domain of a certificate a row has the create ask for beside the one under
/// test.
const SIBLING_DOMAIN: &str = "sibling.example.com";

/// The tags a load balancer of this suite carries itself — a set no change to one
/// of its certificates may touch.
const BALANCER_TAGS: &[TagRow] = &[("owner", Some("network")), ("tier", None)];

/// The tag set the rows that create a certificate with tags use: two keys with
/// values, one key-only tag, and a key carrying a hyphen and a digits-only value.
const CREATED_TAGS: &[TagRow] = &[
    ("env", Some("staging")),
    ("team", Some("edge")),
    ("keep", None),
    ("cost-centre", Some("8080")),
];

/// The tags every row's tag change adds: a key the tagged certificates may
/// already carry (so the merge replaces a value in place), a key none carries,
/// and a key-only tag.
const ADDED_TAGS: &[TagRow] = &[
    ("env", Some("prod")),
    ("rotation", Some("90d")),
    ("audited", None),
];

/// The keys every row's removal lists: one the certificate carries and one no
/// row ever wrote.
const REMOVED_KEYS: &[&str] = &["env", "never-written"];

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// One certificate the table drives.
#[derive(Debug)]
struct Row {
    case: &'static str,
    region: &'static str,
    balancer: &'static str,
    /// A certificate the load balancer also holds, asked for by the create that
    /// made it — so a row can drive the SECOND certificate of a load balancer as
    /// well as its first.
    sibling: Option<&'static str>,
    certificate: &'static str,
    domain: &'static str,
    /// Whether the certificate under test is the one `CreateLoadBalancer`'s own
    /// certificate members asked for, rather than one
    /// `CreateLoadBalancerTlsCertificate` added. That call writes no tags for the
    /// certificate it was asked for, so such a row starts from an empty set.
    created_with_the_load_balancer: bool,
    /// The tags the certificate's create wrote on it.
    created: &'static [TagRow],
}

const ROWS: &[Row] = &[
    Row {
        case: "the certificate CreateLoadBalancer itself asked for, which carries no tags",
        region: DEFAULT_REGION,
        balancer: "tag-lb-with-cert",
        sibling: None,
        certificate: "lb_made_cert",
        domain: "example.com",
        created_with_the_load_balancer: true,
        created: &[],
    },
    Row {
        case: "a certificate created with four tags of its own, one of them key-only",
        region: DEFAULT_REGION,
        balancer: "tag-lb-added-cert",
        sibling: None,
        certificate: "added-cert",
        domain: "shop.example.net",
        created_with_the_load_balancer: false,
        created: CREATED_TAGS,
    },
    Row {
        case: "the SECOND certificate of a load balancer that already holds one",
        region: DEFAULT_REGION,
        balancer: "tag-lb-two-certs",
        sibling: Some("first-cert"),
        certificate: "second-cert-2",
        domain: "api.example.org",
        created_with_the_load_balancer: false,
        created: &[("env", Some("staging"))],
    },
    Row {
        case: "a certificate of a load balancer in another Region",
        region: OTHER_REGION,
        balancer: "tag-lb-elsewhere",
        sibling: None,
        certificate: "elsewhere-cert",
        domain: "eu.example.com",
        created_with_the_load_balancer: false,
        created: CREATED_TAGS,
    },
];

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// Create the load balancer `name` in `region`, carrying [`BALANCER_TAGS`] and —
/// when `certificate` is given — the certificate that create asks for.
async fn create_load_balancer(
    emulator: &Emulator,
    region: &str,
    name: &str,
    certificate: Option<(&str, &str)>,
) {
    let mut body = json!({
        "loadBalancerName": name,
        "instancePort": INSTANCE_PORT,
        "tags": tag_list(BALANCER_TAGS),
    });
    if let Some((certificate_name, domain)) = certificate {
        body["certificateName"] = json!(certificate_name);
        body["certificateDomainName"] = json!(domain);
    }
    emulator
        .ok_at(None, Some(region), "CreateLoadBalancer", body)
        .await;
}

/// Add the certificate `name` for `domain`, carrying `tags`, to the load balancer
/// `balancer` in `region`.
async fn create_certificate(
    emulator: &Emulator,
    region: &str,
    balancer: &str,
    name: &str,
    domain: &str,
    tags: &[TagRow],
) {
    let mut body = json!({
        "loadBalancerName": balancer,
        "certificateName": name,
        "certificateDomainName": domain,
    });
    if !tags.is_empty() {
        body["tags"] = tag_list(tags);
    }
    emulator
        .ok_at(
            None,
            Some(region),
            "CreateLoadBalancerTlsCertificate",
            body,
        )
        .await;
}

/// Stand up the load balancer and the certificates `row` describes.
async fn stand_up(emulator: &Emulator, row: &Row) {
    let created_with_the_balancer = row
        .created_with_the_load_balancer
        .then_some((row.certificate, row.domain))
        .or_else(|| row.sibling.map(|sibling| (sibling, SIBLING_DOMAIN)));
    create_load_balancer(
        emulator,
        row.region,
        row.balancer,
        created_with_the_balancer,
    )
    .await;
    if !row.created_with_the_load_balancer {
        create_certificate(
            emulator,
            row.region,
            row.balancer,
            row.certificate,
            row.domain,
            row.created,
        )
        .await;
    }
}

/// Call `TagResource` for `name` in `region` with the tag list `tags`, requiring
/// success, and answer the one operation-log entry it reports.
async fn tag(emulator: &Emulator, region: &str, name: &str, tags: Value) -> Value {
    let answered = emulator
        .ok_at(
            None,
            Some(region),
            "TagResource",
            json!({"resourceName": name, "tags": tags}),
        )
        .await;
    let operations = answered["operations"].as_array().unwrap_or_else(|| {
        panic!("case tagging {name}: expected an operations array, got {answered:?}")
    });
    assert_eq!(
        operations.len(),
        1,
        "case tagging {name}: expected one entry for the one resource named, got {operations:?}"
    );
    operations[0].clone()
}

/// The tags a resource reports, as `(key, value)` pairs in report order — the
/// shape `resource_tags::want_tags` builds a comparison out of.
type StoredTags = Vec<(String, Option<String>)>;

/// The certificate `name` of the load balancer `balancer` in `region`, as
/// `GetLoadBalancerTlsCertificates` reports it.
async fn read(emulator: &Emulator, region: &str, balancer: &str, name: &str) -> Value {
    let body = emulator
        .ok_at(
            None,
            Some(region),
            "GetLoadBalancerTlsCertificates",
            json!({"loadBalancerName": balancer}),
        )
        .await;
    let certificates = body["tlsCertificates"].as_array().unwrap_or_else(|| {
        panic!("case reading {name}: expected a tlsCertificates array, got {body:?}")
    });
    certificates
        .iter()
        .find(|certificate| certificate["name"].as_str() == Some(name))
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case reading {name}: expected {balancer} in {region} to report it, \
                 got {certificates:?}"
            )
        })
}

/// The tags the certificate `name` of `balancer` carries now.
async fn reported(emulator: &Emulator, region: &str, balancer: &str, name: &str) -> StoredTags {
    reported_tags(&read(emulator, region, balancer, name).await)
}

/// The ARN the certificate `name` of `balancer` reports.
async fn reported_arn(emulator: &Emulator, region: &str, balancer: &str, name: &str) -> String {
    let certificate = read(emulator, region, balancer, name).await;
    certificate["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case reading {name}: expected an arn, got {certificate:?}"))
        .to_string()
}

/// The tags of `held` with `added` merged in by key: a key already held keeps its
/// place and takes the new value, a key not held is appended in request order —
/// what the modelled "tag keys must be unique per resource" leaves behind.
fn merged(held: &[TagRow], added: &[TagRow]) -> StoredTags {
    let mut want = want_tags(held);
    for (key, value) in added {
        let value = value.map(str::to_string);
        match want.iter_mut().find(|(stored_key, _)| stored_key == key) {
            Some(entry) => entry.1 = value,
            None => want.push(((*key).to_string(), value)),
        }
    }
    want
}

/// The tags of `held` with every tag whose key is in `keys` removed.
fn without(held: &[(String, Option<String>)], keys: &[&str]) -> StoredTags {
    held.iter()
        .filter(|(key, _)| !keys.contains(&key.as_str()))
        .cloned()
        .collect()
}

/// A tag list of `count` distinct keys, each carrying a value, as a request
/// carries it — for the cases about how many tags one resource may hold.
fn generated_tags(prefix: &str, count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({"key": format!("{prefix}-{index}"), "value": format!("v{index}")}))
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// RULE: a certificate of a load balancer takes the tags a `TagResource` merges
/// into it and loses exactly the keys an `UntagResource` lists, each change
/// written to its own record and logged against the certificate itself.
#[tokio::test]
async fn a_load_balancer_certificate_takes_and_loses_tags_by_key() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::new();
        stand_up(&emulator, row).await;

        let created = reported(&emulator, row.region, row.balancer, row.certificate).await;
        assert_eq!(
            created,
            want_tags(row.created),
            "case {case}: expected the create's own tags on the certificate, got {created:?}"
        );

        let entry = tag(
            &emulator,
            row.region,
            row.certificate,
            tag_list(ADDED_TAGS),
        )
        .await;
        assert_entry(
            case,
            &entry,
            row.certificate,
            CERTIFICATE_RESOURCE_TYPE,
            row.region,
        );

        let want = merged(row.created, ADDED_TAGS);
        let tagged = reported(&emulator, row.region, row.balancer, row.certificate).await;
        assert_eq!(
            tagged, want,
            "case {case}: expected {want:?} on the certificate after tagging, got {tagged:?}"
        );

        let entry = untag(
            &emulator,
            Some(row.region),
            row.certificate,
            REMOVED_KEYS,
        )
        .await;
        assert_entry(
            case,
            &entry,
            row.certificate,
            CERTIFICATE_RESOURCE_TYPE,
            row.region,
        );

        let left = reported(&emulator, row.region, row.balancer, row.certificate).await;
        assert_eq!(
            left,
            without(&want, REMOVED_KEYS),
            "case {case}: expected only the unlisted keys to survive, got {left:?}"
        );
    }
}

/// RULE: the change reaches the ONE certificate the name addresses — not the load
/// balancer that holds it, not another certificate of that load balancer, and not
/// a certificate of the same name held in another Region.
#[tokio::test]
async fn only_the_named_certificate_changes() {
    let emulator = Emulator::new();
    // One load balancer holding two certificates, and a load balancer in another
    // Region holding a certificate under the SAME name as the one tagged below.
    create_load_balancer(
        &emulator,
        DEFAULT_REGION,
        "shared-lb",
        Some(("twin-cert", "one.example.com")),
    )
    .await;
    create_certificate(
        &emulator,
        DEFAULT_REGION,
        "shared-lb",
        "other-cert",
        "two.example.com",
        CREATED_TAGS,
    )
    .await;
    create_load_balancer(&emulator, OTHER_REGION, "shared-lb-eu", None).await;
    create_certificate(
        &emulator,
        OTHER_REGION,
        "shared-lb-eu",
        "twin-cert",
        "three.example.com",
        CREATED_TAGS,
    )
    .await;

    let entry = tag(
        &emulator,
        DEFAULT_REGION,
        "twin-cert",
        tag_list(ADDED_TAGS),
    )
    .await;
    assert_entry(
        "the first certificate of the shared load balancer",
        &entry,
        "twin-cert",
        CERTIFICATE_RESOURCE_TYPE,
        DEFAULT_REGION,
    );

    let tagged = reported(&emulator, DEFAULT_REGION, "shared-lb", "twin-cert").await;
    assert_eq!(
        tagged,
        merged(&[], ADDED_TAGS),
        "case the named certificate: expected the merged set, got {tagged:?}"
    );
    let sibling = reported(&emulator, DEFAULT_REGION, "shared-lb", "other-cert").await;
    assert_eq!(
        sibling,
        want_tags(CREATED_TAGS),
        "case the other certificate of the same load balancer: expected its create's tags, \
         got {sibling:?}"
    );
    let elsewhere = reported(&emulator, OTHER_REGION, "shared-lb-eu", "twin-cert").await;
    assert_eq!(
        elsewhere,
        want_tags(CREATED_TAGS),
        "case the certificate of that name in {OTHER_REGION}: expected its create's tags, \
         got {elsewhere:?}"
    );

    // The load balancer is a resource of its own: its tags are the ones its
    // create wrote, whatever happened to the certificates it holds.
    for (case, region, name) in [
        ("the load balancer holding the tagged certificate", DEFAULT_REGION, "shared-lb"),
        ("the load balancer in the other Region", OTHER_REGION, "shared-lb-eu"),
    ] {
        let balancer = emulator
            .ok_at(
                None,
                Some(region),
                "GetLoadBalancer",
                json!({"loadBalancerName": name}),
            )
            .await["loadBalancer"]
            .clone();
        let tags = reported_tags(&balancer);
        assert_eq!(
            tags,
            want_tags(BALANCER_TAGS),
            "case {case}: expected the load balancer's own tags untouched, got {tags:?}"
        );
    }
}

/// RULE: a certificate name is scoped to the load balancer that holds it, so
/// where two load balancers of ONE Region each hold a certificate of that name
/// the change addresses the certificate of the first of them in the store's own
/// order — the one record that Region's flat set of active names reports the
/// name for — and leaves the other exactly as it was.
#[tokio::test]
async fn a_name_two_load_balancers_of_one_region_share_addresses_the_first() {
    let emulator = Emulator::new();
    // A load balancer is keyed by its Region together with its name, so "lb-a"
    // is the first of these two in the store's order whichever order they were
    // created in — this creates them the other way round to pin that.
    create_load_balancer(
        &emulator,
        DEFAULT_REGION,
        "lb-b",
        Some(("shared-name", "b.example.com")),
    )
    .await;
    create_load_balancer(
        &emulator,
        DEFAULT_REGION,
        "lb-a",
        Some(("shared-name", "a.example.com")),
    )
    .await;

    let entry = tag(
        &emulator,
        DEFAULT_REGION,
        "shared-name",
        tag_list(ADDED_TAGS),
    )
    .await;
    assert_entry(
        "a certificate name two load balancers of one Region share",
        &entry,
        "shared-name",
        CERTIFICATE_RESOURCE_TYPE,
        DEFAULT_REGION,
    );

    let first = reported(&emulator, DEFAULT_REGION, "lb-a", "shared-name").await;
    assert_eq!(
        first,
        merged(&[], ADDED_TAGS),
        "case the first load balancer of the Region: expected its certificate tagged, \
         got {first:?}"
    );
    let second = reported(&emulator, DEFAULT_REGION, "lb-b", "shared-name").await;
    assert_eq!(
        second,
        want_tags(&[]),
        "case the second load balancer of the Region: expected its certificate as its create \
         left it, got {second:?}"
    );
}

/// RULE: a certificate is addressed in the Region of the load balancer that holds
/// it, so a name no resource of the account holds THERE is refused with
/// Lightsail's `DoesNotExist` envelope and changes nothing.
#[tokio::test]
async fn a_name_the_region_holds_no_certificate_of_is_refused() {
    let emulator = Emulator::new();
    create_load_balancer(&emulator, OTHER_REGION, "away-lb", None).await;
    create_certificate(
        &emulator,
        OTHER_REGION,
        "away-lb",
        "away-cert",
        "away.example.com",
        CREATED_TAGS,
    )
    .await;

    for (case, region, name) in [
        (
            "a certificate held in another Region",
            DEFAULT_REGION,
            "away-cert",
        ),
        (
            "a certificate name nothing was created under",
            OTHER_REGION,
            "absent-cert",
        ),
    ] {
        for (operation, body) in [
            (
                "TagResource",
                json!({"resourceName": name, "tags": tag_list(ADDED_TAGS)}),
            ),
            (
                "UntagResource",
                json!({"resourceName": name, "tagKeys": ["env"]}),
            ),
        ] {
            let (status, answered) = emulator.call_in(Some(region), operation, body).await;
            assert_eq!(
                status, 400,
                "case {case} through {operation}: expected HTTP 400, got {status} {answered:?}"
            );
            assert_eq!(
                error_code(&answered),
                "NotFoundException",
                "case {case} through {operation}: expected the modelled NotFoundException, \
                 got {answered:?}"
            );
            assert_eq!(
                answered["code"].as_str(),
                Some("DoesNotExist"),
                "case {case} through {operation}: expected the DoesNotExist code, \
                 got {answered:?}"
            );
        }
    }

    let kept = reported(&emulator, OTHER_REGION, "away-lb", "away-cert").await;
    assert_eq!(
        kept,
        want_tags(CREATED_TAGS),
        "case the refused calls: expected the certificate's tags untouched, got {kept:?}"
    );
}

/// RULE: `resourceArn`, when the request carries one, is the ARN of the resource
/// `resourceName` names — the certificate's OWN ARN, which names its own resource
/// type and its own minted id, is served, and any other resource's is refused
/// `InvalidInputException` (HTTP 400) and writes nothing.
#[tokio::test]
async fn an_arn_the_request_carries_must_be_the_certificates_own() {
    let emulator = Emulator::new();
    create_load_balancer(&emulator, DEFAULT_REGION, "arn-lb", None).await;
    create_certificate(
        &emulator,
        DEFAULT_REGION,
        "arn-lb",
        "arn-cert",
        "arn.example.com",
        CREATED_TAGS,
    )
    .await;
    create_certificate(
        &emulator,
        DEFAULT_REGION,
        "arn-lb",
        "arn-cert-other",
        "other.arn.example.com",
        CREATED_TAGS,
    )
    .await;

    let balancer = emulator
        .ok_at(
            None,
            Some(DEFAULT_REGION),
            "GetLoadBalancer",
            json!({"loadBalancerName": "arn-lb"}),
        )
        .await["loadBalancer"]
        .clone();
    let balancer_arn = balancer["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case reading arn-lb: expected an arn, got {balancer:?}"))
        .to_string();
    let own_arn = reported_arn(&emulator, DEFAULT_REGION, "arn-lb", "arn-cert").await;
    let other_arn = reported_arn(&emulator, DEFAULT_REGION, "arn-lb", "arn-cert-other").await;

    // The certificate's ARN is its own: this account's, in the Region of the load
    // balancer that holds it, naming the certificate's resource type and the id
    // minted for it — never the load balancer's.
    let want_prefix = format!(
        "arn:aws:lightsail:{DEFAULT_REGION}:{}:{CERTIFICATE_RESOURCE_TYPE}/",
        engine::EMULATOR_ACCOUNT_ID
    );
    assert!(
        own_arn.starts_with(&want_prefix),
        "case the certificate's ARN: expected it to start with {want_prefix:?}, got {own_arn:?}"
    );
    let id = own_arn.trim_start_matches(&want_prefix);
    assert!(
        is_uuid_shaped(id),
        "case the certificate's ARN: expected a uuid-shaped id, got {id:?}"
    );
    assert_ne!(
        own_arn, other_arn,
        "case two certificates of one load balancer: expected each its own ARN, got {own_arn:?}"
    );

    for (case, arn) in [
        ("the load balancer's ARN", balancer_arn),
        ("another certificate's ARN", other_arn),
    ] {
        let (status, answered) = emulator
            .call_in(
                Some(DEFAULT_REGION),
                "TagResource",
                json!({
                    "resourceName": "arn-cert",
                    "resourceArn": arn,
                    "tags": tag_list(ADDED_TAGS),
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "InvalidInputException",
            "case {case}: expected the modelled InvalidInputException, got {answered:?}"
        );
        let untouched = reported(&emulator, DEFAULT_REGION, "arn-lb", "arn-cert").await;
        assert_eq!(
            untouched,
            want_tags(CREATED_TAGS),
            "case {case}: expected the refusal to write nothing, got {untouched:?}"
        );
    }

    let (status, answered) = emulator
        .call_in(
            Some(DEFAULT_REGION),
            "TagResource",
            json!({
                "resourceName": "arn-cert",
                "resourceArn": own_arn,
                "tags": tag_list(ADDED_TAGS),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case its own ARN: expected it to be served, got {status} {answered:?}"
    );
    let tagged = reported(&emulator, DEFAULT_REGION, "arn-lb", "arn-cert").await;
    assert_eq!(
        tagged,
        merged(CREATED_TAGS, ADDED_TAGS),
        "case its own ARN: expected the merged set, got {tagged:?}"
    );
}

/// RULE: the bound is on what the RESOURCE ends up carrying, so a certificate
/// takes tags up to the 50 one Lightsail resource may hold, a request that would
/// take it past them is refused and writes nothing, and a removal makes room
/// again.
#[tokio::test]
async fn a_certificates_tag_set_is_bounded_by_the_fifty_a_resource_may_carry() {
    let emulator = Emulator::new();
    create_load_balancer(&emulator, DEFAULT_REGION, "bound-lb", None).await;
    create_certificate(
        &emulator,
        DEFAULT_REGION,
        "bound-lb",
        "bound-cert",
        "bound.example.com",
        &[("env", Some("staging"))],
    )
    .await;

    // Up to the bound: one tag from the create plus the rest of the fifty.
    tag(
        &emulator,
        DEFAULT_REGION,
        "bound-cert",
        generated_tags("fill", MAX_TAGS - 1),
    )
    .await;
    let filled = reported(&emulator, DEFAULT_REGION, "bound-lb", "bound-cert").await;
    assert_eq!(
        filled.len(),
        MAX_TAGS,
        "case a certificate filled to the bound: expected {MAX_TAGS} tags, got {filled:?}"
    );

    // One key past it, refused, and nothing written.
    let (status, answered) = emulator
        .call_in(
            Some(DEFAULT_REGION),
            "TagResource",
            json!({"resourceName": "bound-cert", "tags": generated_tags("over", 1)}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case one tag past the bound: expected HTTP 400, got {status} {answered:?}"
    );
    assert_eq!(
        error_code(&answered),
        "InvalidInputException",
        "case one tag past the bound: expected the modelled InvalidInputException, \
         got {answered:?}"
    );
    let unchanged = reported(&emulator, DEFAULT_REGION, "bound-lb", "bound-cert").await;
    assert_eq!(
        unchanged, filled,
        "case one tag past the bound: expected the refusal to write nothing, got {unchanged:?}"
    );

    // A key the certificate already carries is no new tag, so the same request
    // that was refused is served once a removal has made room.
    untag(&emulator, Some(DEFAULT_REGION), "bound-cert", &["fill-0"]).await;
    tag(
        &emulator,
        DEFAULT_REGION,
        "bound-cert",
        generated_tags("over", 1),
    )
    .await;
    let refilled = reported(&emulator, DEFAULT_REGION, "bound-lb", "bound-cert").await;
    assert_eq!(
        refilled.len(),
        MAX_TAGS,
        "case a removal making room: expected {MAX_TAGS} tags again, got {refilled:?}"
    );
    assert!(
        refilled
            .iter()
            .any(|(key, _)| key == "over-0")
            && !refilled.iter().any(|(key, _)| key == "fill-0"),
        "case a removal making room: expected the removed key gone and the new one held, \
         got {refilled:?}"
    );
}
