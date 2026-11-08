// SPDX-License-Identifier: CC0-1.0

//! Load balancer attributes: what a load balancer reports before anything writes one,
//! what a write stores and answers, and what each refusal says — over the wire, through
//! the router.
//!
//! The rules these tables pin:
//!
//! * A load balancer reports every attribute the model documents for a load balancer of
//!   its family, carrying the value it is held at until a write changes it
//!   (`smithy_query(query="doc",shape="LoadBalancerAttribute",member="Key")` is the only
//!   catalog of them, since the Smithy shape itself is an unconstrained key/value pair) —
//!   including the string-valued log destinations, which the collection carries at the
//!   empty string.
//! * `ModifyLoadBalancerAttributes` persists exactly the attributes the request names,
//!   leaves every attribute it does not name as it was, and answers with the load
//!   balancer's WHOLE attribute collection: the same list, in the same order,
//!   `DescribeLoadBalancerAttributes` then reports.
//! * A key the model does not document, a key another family's load balancers carry, a
//!   value outside an attribute's documented domain, and a change to an attribute that
//!   documentation fixes are each refused with the operation's own
//!   `InvalidConfigurationRequest`, and a refused write stores nothing.
//!
//! The wire harness (`emulator`, `call`, `action`, `p`, `members`, `first_text`) is the
//! one `provisioning_loop` already owns; nothing here restates it.

use crate::provisioning_loop::{
    action, attribute_sequence, call, emulator, first_text, members, p, Reply,
};
use crate::target_group_attributes_and_modification::unescaped;
use smithy_server_runtime::MultiServiceRouter;
use std::collections::BTreeMap;

/// One row of the default table: the case name, the fields the load balancer is created
/// with, and the whole attribute set it must then report.
type DefaultCase = (
    &'static str,
    Vec<(&'static str, &'static str)>,
    Vec<(&'static str, &'static str)>,
);

/// One row of the write table: the case name, the fields the load balancer is created
/// with, and the attributes one request writes onto it in the order it names them.
type WriteCase = (
    &'static str,
    Vec<(&'static str, &'static str)>,
    Vec<(&'static str, &'static str)>,
);

/// One row of the refusal table: the case name, the fields the load balancer is created
/// with, the attribute the request writes, and the awsQuery error code expected.
type RefusalCase = (
    &'static str,
    Vec<(&'static str, &'static str)>,
    (&'static str, &'static str),
    &'static str,
);

/// The awsQuery error code of `InvalidConfigurationRequestException`
/// (`smithy_query(query="operation",operation="ModifyLoadBalancerAttributes")`).
pub(crate) const INVALID_CONFIGURATION: &str = "InvalidConfigurationRequest";

/// The code Elastic Load Balancing answers an input value it cannot accept with, for a
/// rule its model documents in prose and declares no error shape for.
pub(crate) const VALIDATION_ERROR: &str = "ValidationError";

/// Create a load balancer named `name` with `fields` beside it, and answer its ARN.
///
/// Two subnets by default, because an Application Load Balancer must be placed in at
/// least two Availability Zones; a row that needs other placement passes its own
/// `Subnets.member.N` fields, which replace these.
///
/// `pub(crate)` because the sibling suite
/// (`all/load_balancer_network_capacity_and_policy.rs`) makes load balancers of every
/// family the same way: ONE creator for the crate, so the two suites cannot drift on how
/// a load balancer is made.
pub(crate) async fn create_load_balancer(
    router: &MultiServiceRouter,
    name: &str,
    fields: &[(&str, &str)],
) -> String {
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", name));
    if !fields.iter().any(|(key, _)| key.starts_with("Subnets")) {
        form.push(p("Subnets.member.1", "subnet-11111111"));
        form.push(p("Subnets.member.2", "subnet-22222222"));
    }
    for (key, value) in fields {
        form.push(p(key, value));
    }
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case create {name}: expected 200, got {} body {}",
        reply.status, reply.body
    );
    reply
        .text("LoadBalancerArn")
        .unwrap_or_else(|| panic!("case create {name}: no LoadBalancerArn in {}", reply.body))
}

/// Every key/value pair in the first `Attributes` list of `body`.
fn attribute_pairs(body: &str) -> BTreeMap<String, String> {
    members(body, "Attributes")
        .iter()
        .map(|member| {
            (
                first_text(member, "Key").unwrap_or_default(),
                first_text(member, "Value").unwrap_or_default(),
            )
        })
        .collect()
}

/// What `DescribeLoadBalancerAttributes` reports for `arn`.
pub(crate) async fn described_attributes(
    router: &MultiServiceRouter,
    arn: &str,
) -> BTreeMap<String, String> {
    let mut form = action("DescribeLoadBalancerAttributes");
    form.push(p("LoadBalancerArn", arn));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case describe attributes of {arn}: expected 200, got {} body {}",
        reply.status, reply.body
    );
    attribute_pairs(&reply.body)
}

/// What `DescribeLoadBalancerAttributes` reports for `arn`, in the order it reports it.
async fn described_sequence(router: &MultiServiceRouter, arn: &str) -> Vec<(String, String)> {
    let mut form = action("DescribeLoadBalancerAttributes");
    form.push(p("LoadBalancerArn", arn));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case describe attributes of {arn}: expected 200, got {} body {}",
        reply.status, reply.body
    );
    attribute_sequence(&reply.body)
}

/// Write `writes` onto the load balancer `arn` names and answer the reply, whatever it
/// is. The form numbers the members in the order given, which is the order the reply's
/// `Attributes` list must come back in.
pub(crate) async fn write_attributes(
    router: &MultiServiceRouter,
    arn: &str,
    writes: &[(&str, &str)],
) -> Reply {
    let mut form = action("ModifyLoadBalancerAttributes");
    form.push(p("LoadBalancerArn", arn));
    for (index, (key, value)) in writes.iter().enumerate() {
        let position = index + 1;
        form.push(p(&format!("Attributes.member.{position}.Key"), key));
        form.push(p(&format!("Attributes.member.{position}.Value"), value));
    }
    call(router, &form).await
}

/// `DeleteLoadBalancer` for one ARN, and the reply whatever it is.
async fn delete_load_balancer(router: &MultiServiceRouter, arn: &str) -> Reply {
    let mut form = action("DeleteLoadBalancer");
    form.push(p("LoadBalancerArn", arn));
    call(router, &form).await
}

/// `pairs` as the map a read must report.
fn expected_map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// `writes` as the ordered list a write's answer must carry.
fn expected_sequence(writes: &[(&str, &str)]) -> Vec<(String, String)> {
    writes
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// `before` with `writes` applied, which is the whole set a later read must report.
fn merged(before: &BTreeMap<String, String>, writes: &[(&str, &str)]) -> BTreeMap<String, String> {
    let mut expected = before.clone();
    for (key, value) in writes {
        expected.insert((*key).to_string(), (*value).to_string());
    }
    expected
}

// ---------------------------------------------------------------------------
// Defaults, per load balancer family and scheme.
// ---------------------------------------------------------------------------

/// The WHOLE attribute collection a load balancer reports before anything writes one,
/// and the value each key is carried at, follow the model's own
/// `LoadBalancerAttribute$Key` documentation — which groups the keys by the family that
/// carries them, gives `load_balancing.cross_zone.enabled` a value that follows the
/// family, and states no value for the string-valued log destinations, which the
/// collection carries at the empty string.
///
/// One row per family the catalog names, plus an INTERNAL load balancer, whose scheme
/// changes no key of the collection.
#[tokio::test]
async fn a_load_balancer_reports_the_documented_default_of_every_attribute_its_family_carries() {
    // "The following attributes are supported by only Application Load Balancers",
    // beside the two every family carries and the three "supported by both Application
    // Load Balancers and Network Load Balancers".
    let application: Vec<(&str, &str)> = vec![
        ("access_logs.s3.enabled", "false"),
        ("access_logs.s3.bucket", ""),
        ("access_logs.s3.prefix", ""),
        ("health_check_logs.s3.enabled", "false"),
        ("health_check_logs.s3.bucket", ""),
        ("health_check_logs.s3.prefix", ""),
        ("idle_timeout.timeout_seconds", "60"),
        ("deletion_protection.enabled", "false"),
        ("routing.http2.enabled", "true"),
        ("routing.http.drop_invalid_header_fields.enabled", "false"),
        ("routing.http.xff_client_port.enabled", "false"),
        ("routing.http.xff_header_processing.mode", "append"),
        ("routing.http.preserve_host_header.enabled", "false"),
        (
            "routing.http.x_amzn_tls_version_and_cipher_suite.enabled",
            "false",
        ),
        ("routing.http.desync_mitigation_mode", "defensive"),
        ("load_balancing.cross_zone.enabled", "true"),
        ("client_keep_alive.seconds", "3600"),
        ("waf.fail_open.enabled", "false"),
        ("zonal_shift.config.enabled", "false"),
        ("connection_logs.s3.enabled", "false"),
        ("connection_logs.s3.bucket", ""),
        ("connection_logs.s3.prefix", ""),
    ];

    let cases: Vec<DefaultCase> = vec![
        (
            "an internet-facing application load balancer",
            vec![],
            application.clone(),
        ),
        (
            // The scheme decides no key of the collection: the same set comes back.
            "an internal application load balancer",
            vec![("Scheme", "internal")],
            application.clone(),
        ),
        (
            "a network load balancer",
            vec![("Type", "network")],
            vec![
                ("access_logs.s3.enabled", "false"),
                ("access_logs.s3.bucket", ""),
                ("access_logs.s3.prefix", ""),
                ("deletion_protection.enabled", "false"),
                // "The default for Network Load Balancers and Gateway Load Balancers is
                // false."
                ("load_balancing.cross_zone.enabled", "false"),
                ("zonal_shift.config.enabled", "false"),
                ("secondary_ips.auto_assigned.per_subnet", "0"),
            ],
        ),
        (
            "a gateway load balancer",
            vec![("Type", "gateway")],
            vec![
                ("deletion_protection.enabled", "false"),
                ("load_balancing.cross_zone.enabled", "false"),
            ],
        ),
    ];

    let (router, _engine) = emulator();

    for (index, (case, fields, expected)) in cases.iter().enumerate() {
        let name = format!("lb-attr-default-{index}");
        let arn = create_load_balancer(&router, &name, fields).await;
        let reported = described_attributes(&router, &arn).await;
        assert_eq!(
            reported,
            expected_map(expected),
            "case {case:?}: expected the whole documented collection, got {reported:?}"
        );
        // ...and in the order the service reports it in, which is the catalog's and
        // neither alphabetical nor the request's.
        let sequence = described_sequence(&router, &arn).await;
        assert_eq!(
            sequence,
            expected_sequence(expected),
            "case {case:?}: expected the collection in catalog order, got {sequence:?}"
        );
    }
}

/// One row of the uncarried-attribute table: the case name, the fields the load balancer
/// is created with, and the value one request writes for the uncarried key.
type UncarriedCase = (&'static str, Vec<(&'static str, &'static str)>, &'static str);

/// An attribute the collection does not carry is still writable, and a write is what puts
/// it there: `ipv6.deny_all_igw_traffic` is reported by neither a read nor a write of a
/// fresh load balancer of either family that accepts it, and by both once written.
#[tokio::test]
async fn an_attribute_the_collection_does_not_carry_is_reported_only_once_written() {
    let key = "ipv6.deny_all_igw_traffic";
    // The documentation lists it for "both Application Load Balancers and Network Load
    // Balancers", and makes its value follow the scheme, so both families and both
    // schemes are asked.
    let cases: Vec<UncarriedCase> = vec![
        ("an internet-facing application load balancer", vec![], "true"),
        (
            "an internal application load balancer",
            vec![("Scheme", "internal")],
            "false",
        ),
        ("a network load balancer", vec![("Type", "network")], "true"),
    ];

    let (router, _engine) = emulator();

    for (index, (case, fields, value)) in cases.iter().enumerate() {
        let name = format!("lb-attr-uncarried-{index}");
        let arn = create_load_balancer(&router, &name, fields).await;
        let before = described_attributes(&router, &arn).await;
        assert!(
            !before.contains_key(key),
            "case {case:?}: expected no {key} before a write, got {before:?}"
        );

        let reply = write_attributes(&router, &arn, &[(key, value)]).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200, got {} body {}",
            reply.status, reply.body
        );
        let after = described_attributes(&router, &arn).await;
        assert_eq!(
            after.get(key).map(String::as_str),
            Some(*value),
            "case {case:?}: expected the written value reported, got {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Writes: what lands, what comes back, and in which order.
// ---------------------------------------------------------------------------

/// A write answers with the load balancer's WHOLE attribute collection — the same list,
/// in the same order, a read then reports — carrying the values the request wrote beside
/// every attribute it left alone.
///
/// The rows differ along what the rule generalises over: which family's attributes a load
/// balancer carries, how many attributes one request names and in which order, the kind
/// of domain each attribute has (a boolean, an enumerated mode, an integer at a published
/// bound, a free-text bucket name the model states no value for), and a write that
/// restates the value an attribute is fixed at.
#[tokio::test]
async fn a_write_answers_the_whole_collection_a_read_reports() {
    let three: Vec<(&str, &str)> = vec![
        ("idle_timeout.timeout_seconds", "120"),
        ("routing.http2.enabled", "false"),
        ("waf.fail_open.enabled", "true"),
    ];
    let mut reversed = three.clone();
    reversed.reverse();

    let cases: Vec<WriteCase> = vec![
        (
            "one boolean on an application load balancer",
            vec![],
            vec![("deletion_protection.enabled", "true")],
        ),
        ("three attributes in catalog order", vec![], three),
        ("the same three in the opposite order", vec![], reversed),
        (
            "an enumerated mode and the integer bounds its documentation publishes",
            vec![],
            vec![
                ("routing.http.xff_header_processing.mode", "preserve"),
                ("idle_timeout.timeout_seconds", "1"),
                ("client_keep_alive.seconds", "604800"),
            ],
        ),
        (
            "the log attributes, including two the model states no value for",
            vec![],
            vec![
                ("access_logs.s3.enabled", "true"),
                ("access_logs.s3.bucket", "logs-bucket"),
                ("access_logs.s3.prefix", "alb/one"),
            ],
        ),
        (
            "a write restating the value cross-zone is fixed at on this family",
            vec![],
            vec![("load_balancing.cross_zone.enabled", "true")],
        ),
        (
            "a network load balancer's own attributes",
            vec![("Type", "network")],
            vec![
                ("load_balancing.cross_zone.enabled", "true"),
                ("dns_record.client_routing_policy", "availability_zone_affinity"),
                ("secondary_ips.auto_assigned.per_subnet", "7"),
            ],
        ),
        (
            "a gateway load balancer's own attributes",
            vec![("Type", "gateway")],
            vec![
                ("load_balancing.cross_zone.enabled", "true"),
                ("deletion_protection.enabled", "true"),
            ],
        ),
    ];

    let (router, _engine) = emulator();

    for (index, (case, fields, writes)) in cases.iter().enumerate() {
        // A fresh load balancer per row, so one row's write never decides the next
        // row's answer.
        let name = format!("lb-attr-write-{index}");
        let arn = create_load_balancer(&router, &name, fields).await;
        let before = described_attributes(&router, &arn).await;

        let reply = write_attributes(&router, &arn, writes).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200, got {} body {}",
            reply.status, reply.body
        );

        // The answer is the whole collection, carrying what the request wrote: every key
        // the load balancer reported before the write is in it, with the written values
        // replacing their own.
        let answered = attribute_sequence(&reply.body);
        let after = described_attributes(&router, &arn).await;
        assert_eq!(
            after,
            merged(&before, writes),
            "case {case:?}: expected the written values beside every untouched attribute, got \
             {after:?}"
        );
        assert_eq!(
            answered,
            described_sequence(&router, &arn).await,
            "case {case:?}: expected the write to answer the collection a read reports, got \
             {answered:?}"
        );
    }
}

/// Writes compose: each one replaces only the keys it names, the LAST write of a key is
/// what a read reports, and re-sending a write a load balancer already carries changes
/// nothing.
#[tokio::test]
async fn repeated_writes_replace_only_the_keys_they_name_and_are_idempotent() {
    let (router, _engine) = emulator();
    let arn = create_load_balancer(&router, "lb-attr-sequence", &[]).await;

    let defaults = described_attributes(&router, &arn).await;
    assert_eq!(
        defaults
            .get("idle_timeout.timeout_seconds")
            .map(String::as_str),
        Some("60"),
        "case setup: expected the documented default before any write, got {defaults:?}"
    );

    let steps: Vec<(&str, Vec<(&str, &str)>)> = vec![
        (
            "the first write",
            vec![("idle_timeout.timeout_seconds", "4000")],
        ),
        ("another key", vec![("waf.fail_open.enabled", "true")]),
        (
            "replacing the first key",
            vec![("idle_timeout.timeout_seconds", "30")],
        ),
        (
            "re-sending the replacement",
            vec![("idle_timeout.timeout_seconds", "30")],
        ),
    ];

    let mut expected = defaults.clone();
    for (case, writes) in &steps {
        let reply = write_attributes(&router, &arn, writes).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200, got {} body {}",
            reply.status, reply.body
        );
        expected = merged(&expected, writes);
        let after = described_attributes(&router, &arn).await;
        assert_eq!(
            after, expected,
            "case {case:?}: expected the cumulative set, got {after:?}"
        );
    }

    // Every key the load balancer started with is still reported, and no key appeared: a
    // write replaces values, it never changes which attributes a read answers.
    let after = described_attributes(&router, &arn).await;
    let reported_keys: Vec<&String> = after.keys().collect();
    let original_keys: Vec<&String> = defaults.keys().collect();
    assert_eq!(
        reported_keys, original_keys,
        "case the whole sequence: expected the same attribute keys reported, got \
         {reported_keys:?} against {original_keys:?}"
    );
}

// ---------------------------------------------------------------------------
// Refusals, and what a refused write leaves behind.
// ---------------------------------------------------------------------------

/// An attribute this service cannot store is refused and the load balancer is left exactly
/// as it was. A key the service defines no attribute under, a value outside the
/// attribute's documented domain and a change to a value that documentation fixes are each
/// a value the request has to fix (`ValidationError`); a key the addressed load balancer's
/// TYPE does not carry is classified by that type — an Application Load Balancer answers
/// the operation's own `InvalidConfigurationRequest`, a Network or Gateway Load Balancer
/// answers `ValidationError`.
///
/// The rows span every one of them: a key no documentation lists, a key another family's
/// load balancers carry (in both directions, so both classes of that refusal are driven), a
/// value outside a documented domain — one past each published bound of an integer range and
/// a word outside an enumerated set — and a change to the attribute whose documentation says
/// it "can't be changed".
#[tokio::test]
async fn an_attribute_this_service_cannot_store_is_refused_and_changes_nothing() {
    let cases: Vec<RefusalCase> = vec![
        (
            "a key no documentation lists",
            vec![],
            ("routing.http.invented_mode", "on"),
            VALIDATION_ERROR,
        ),
        (
            "a network load balancer's attribute on an application load balancer",
            vec![],
            ("dns_record.client_routing_policy", "any_availability_zone"),
            INVALID_CONFIGURATION,
        ),
        (
            "an application load balancer's attribute on a network load balancer",
            vec![("Type", "network")],
            ("routing.http2.enabled", "false"),
            VALIDATION_ERROR,
        ),
        (
            "an attribute of both other families on a gateway load balancer",
            vec![("Type", "gateway")],
            ("access_logs.s3.enabled", "true"),
            VALIDATION_ERROR,
        ),
        (
            "a boolean that is neither true nor false",
            vec![],
            ("deletion_protection.enabled", "yes"),
            VALIDATION_ERROR,
        ),
        (
            "one below the published lower bound of an integer range",
            vec![],
            ("idle_timeout.timeout_seconds", "0"),
            VALIDATION_ERROR,
        ),
        (
            "one above the published upper bound of an integer range",
            vec![],
            ("idle_timeout.timeout_seconds", "4001"),
            VALIDATION_ERROR,
        ),
        (
            "one above the published upper bound of the secondary address range",
            vec![("Type", "network")],
            ("secondary_ips.auto_assigned.per_subnet", "8"),
            VALIDATION_ERROR,
        ),
        (
            "a word outside an enumerated set",
            vec![],
            ("routing.http.xff_header_processing.mode", "rewrite"),
            VALIDATION_ERROR,
        ),
        (
            "changing the attribute whose documentation fixes it",
            vec![],
            ("load_balancing.cross_zone.enabled", "false"),
            VALIDATION_ERROR,
        ),
    ];

    let (router, _engine) = emulator();

    for (index, (case, fields, (key, value), code)) in cases.iter().enumerate() {
        let name = format!("lb-attr-refusal-{index}");
        let arn = create_load_balancer(&router, &name, fields).await;
        let before = described_attributes(&router, &arn).await;

        let reply = write_attributes(&router, &arn, &[(key, value)]).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, *code),
            "case {case:?}: expected 400 {code}, got {} body {}",
            reply.status,
            reply.body
        );
        let message = reply.text("Message").map(|text| unescaped(&text));
        assert!(
            message.as_deref().is_some_and(|text| text.contains(key)),
            "case {case:?}: the refusal must name the attribute, got {message:?}"
        );

        let after = described_attributes(&router, &arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused write must store nothing, got {after:?}"
        );
    }
}

/// One bad attribute refuses the WHOLE request: the attributes beside it in the same
/// request are not stored either.
#[tokio::test]
async fn a_request_carrying_one_bad_attribute_stores_none_of_them() {
    let (router, _engine) = emulator();
    let arn = create_load_balancer(&router, "lb-attr-atomic", &[]).await;
    let before = described_attributes(&router, &arn).await;

    let writes = [
        ("deletion_protection.enabled", "true"),
        ("idle_timeout.timeout_seconds", "4001"),
        ("waf.fail_open.enabled", "true"),
    ];
    let reply = write_attributes(&router, &arn, &writes).await;
    assert_eq!(
        (reply.status, reply.error_code().as_str()),
        (400, VALIDATION_ERROR),
        "case a good attribute beside a bad one: expected 400 {VALIDATION_ERROR}, got {} \
         body {}",
        reply.status,
        reply.body
    );

    let after = described_attributes(&router, &arn).await;
    assert_eq!(
        after, before,
        "case a good attribute beside a bad one: nothing in the request may be stored, got \
         {after:?}"
    );
}

// ---------------------------------------------------------------------------
// What an attribute DOES: deletion protection.
// ---------------------------------------------------------------------------

/// `deletion_protection.enabled` is not just reported: "You can't delete a load balancer
/// if deletion protection is enabled", so while it is on `DeleteLoadBalancer` is refused
/// with the modelled `OperationNotPermitted` and the load balancer stays, and once it is
/// turned off the same delete succeeds.
///
/// One row per family that carries the attribute, which its documentation lists under
/// "supported by all load balancers".
#[tokio::test]
async fn deletion_protection_refuses_the_delete_until_it_is_turned_off() {
    /// The awsQuery code of `OperationNotPermittedException`
    /// (`smithy_query(query="operation",operation="DeleteLoadBalancer")`).
    const OPERATION_NOT_PERMITTED: &str = "OperationNotPermitted";

    let cases: Vec<(&str, Vec<(&str, &str)>)> = vec![
        ("an application load balancer", vec![]),
        ("a network load balancer", vec![("Type", "network")]),
        ("a gateway load balancer", vec![("Type", "gateway")]),
    ];

    let (router, _engine) = emulator();

    for (index, (case, fields)) in cases.iter().enumerate() {
        let name = format!("lb-attr-protected-{index}");
        let arn = create_load_balancer(&router, &name, fields).await;

        // Off by default: the delete would succeed, so the protection is turned on first.
        let reply = write_attributes(&router, &arn, &[("deletion_protection.enabled", "true")]).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected the protection to be stored, got {} body {}",
            reply.status, reply.body
        );

        let refused = delete_load_balancer(&router, &arn).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, OPERATION_NOT_PERMITTED),
            "case {case:?}: expected 400 {OPERATION_NOT_PERMITTED}, got {} body {}",
            refused.status,
            refused.body
        );
        let message = refused.text("Message").map(|text| unescaped(&text));
        assert!(
            message
                .as_deref()
                .is_some_and(|text| text.contains(arn.as_str())
                    && text.contains("deletion protection is enabled")),
            "case {case:?}: the refusal must name the load balancer and the rule, got {message:?}"
        );
        // Nothing was removed: the load balancer still answers its attributes.
        let still_there = described_attributes(&router, &arn).await;
        assert_eq!(
            still_there
                .get("deletion_protection.enabled")
                .map(String::as_str),
            Some("true"),
            "case {case:?}: a refused delete must leave the load balancer, got {still_there:?}"
        );

        let reply =
            write_attributes(&router, &arn, &[("deletion_protection.enabled", "false")]).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected the protection to be turned off, got {} body {}",
            reply.status, reply.body
        );
        let deleted = delete_load_balancer(&router, &arn).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected the delete to succeed once unprotected, got {} body {}",
            deleted.status, deleted.body
        );
        // And it is gone: its attributes are no longer there to read.
        let mut form = action("DescribeLoadBalancerAttributes");
        form.push(p("LoadBalancerArn", &arn));
        let gone = call(&router, &form).await;
        assert_eq!(
            (gone.status, gone.error_code().as_str()),
            (400, "LoadBalancerNotFound"),
            "case {case:?}: expected the load balancer to be gone, got {} body {}",
            gone.status,
            gone.body
        );
    }
}
