// SPDX-License-Identifier: CC0-1.0

//! The Application Load Balancer DATA plane, end to end over the wire: what a client that
//! sends traffic to the name a load balancer publishes gets back.
//!
//! The rule under test: **a request a client addresses to an Application Load Balancer is
//! answered out of the listener it reaches — the actions of the first rule, in ascending
//! priority order, whose conditions are ALL met, and the listener's default actions when
//! no rule's are.** A `fixed-response` action answers its own status code, content type
//! and message body; a `redirect` answers its status with the `Location` its components
//! build; a `forward` names targets this emulator has no server behind, so it answers the
//! status that condition earns rather than a body invented for a backend that never ran.
//!
//! Every case drives the real wire path — an HTTP request through the multi-service router
//! into the mounted service, carrying no AWS credential, exactly as a client of the
//! published `DNSName` sends it — and the configuration it is answered from is written
//! through the control plane's own operations (`CreateLoadBalancer`, `CreateTargetGroup`,
//! `CreateListener`, `CreateRule`, `RegisterTargets`), so nothing here is served from a
//! record a test invented.
//!
//! The rows span what the rule generalises over: each documented condition field (given
//! directly and through its own configuration shape), a matching and a non-matching value
//! for each, two rules whose patterns overlap at different priorities, a request no rule
//! matches, each terminating action type, an action this emulator cannot perform, the four
//! addressings a client can reach a load balancer by, and a load balancer the clock still
//! owes its move out of `provisioning`.
//!
//! The clock-owed moves (a load balancer's to `active`, a registration's to `healthy`) are
//! placed on either side of now in the store rather than slept for, exactly as
//! `provisioning_loop::seed_trust_store` places a trust store's.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::ComAmazonawsElasticloadbalancingv2ElasticloadbalancingV10State as ElbState;
use smithy_server_runtime::MultiServiceRouter;

use crate::provisioning_loop::{action, call, create_http_target_group, emulator, members, p};

/// The host a client addresses the emulator's own endpoint by, which is the only address a
/// client on the machine this emulator runs on can reach — and so the host the three
/// non-published addressings are read on.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// The body the listener's DEFAULT action answers with, which is what a request no rule
/// matches gets.
const DEFAULT_BODY: &str = "no-match";

/// One answered request: the status line, the headers asserted on, and the body verbatim.
pub(crate) struct Answered {
    pub(crate) status: u16,
    pub(crate) headers: HashMap<String, String>,
    pub(crate) body: String,
}

impl Answered {
    /// One response header, lower-cased as HTTP names compare.
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

/// Issue one data-plane request: no credential, no body, `headers` as the client sent
/// them.
pub(crate) async fn send(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    target: &str,
    headers: &[(&str, &str)],
) -> Answered {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://{host}{target}"))
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(bytes::Bytes::new())
        .expect("the data-plane request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_ascii_lowercase(), value.to_owned()))
        })
        .collect();
    let body = String::from_utf8_lossy(&response.into_body()).into_owned();
    Answered {
        status,
        headers,
        body,
    }
}

/// Create a load balancer and answer its ARN, its name and the DNS name it published.
pub(crate) async fn create_load_balancer(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", name));
    form.push(p("Subnets.member.1", "subnet-11111111"));
    form.push(p("Subnets.member.2", "subnet-22222222"));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case create load balancer {name}: expected 200, got {} body {}",
        reply.status, reply.body
    );
    let arn = reply
        .text("LoadBalancerArn")
        .unwrap_or_else(|| panic!("case create {name}: no LoadBalancerArn in {}", reply.body));
    let dns_name = reply
        .text("DNSName")
        .unwrap_or_else(|| panic!("case create {name}: no DNSName in {}", reply.body));
    (arn, dns_name)
}

/// Create the HTTP:80 listener whose default action is the fixed response above, and
/// answer its ARN.
async fn create_listener(router: &MultiServiceRouter, lb_arn: &str) -> String {
    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", lb_arn));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", "80"));
    form.push(p("DefaultActions.member.1.Type", "fixed-response"));
    form.push(p(
        "DefaultActions.member.1.FixedResponseConfig.StatusCode",
        "404",
    ));
    form.push(p(
        "DefaultActions.member.1.FixedResponseConfig.MessageBody",
        DEFAULT_BODY,
    ));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case create listener: expected 200, got {} body {}",
        reply.status, reply.body
    );
    reply
        .text("ListenerArn")
        .unwrap_or_else(|| panic!("case create listener: no ListenerArn in {}", reply.body))
}

/// Add one rule at `priority` carrying `fields`, which must be accepted.
async fn add_rule(
    router: &MultiServiceRouter,
    case: &str,
    listener_arn: &str,
    priority: i32,
    fields: &[(String, String)],
) {
    let mut form = action("CreateRule");
    form.push(p("ListenerArn", listener_arn));
    form.push(p("Priority", &priority.to_string()));
    form.extend(fields.iter().cloned());
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case rule {case}: expected 200, got {} body {}",
        reply.status, reply.body
    );
}

/// A `fixed-response` action's fields, under `prefix`.
fn fixed_response(status: &str, body: &str, content_type: Option<&str>) -> Vec<(String, String)> {
    let mut fields = vec![
        p("Actions.member.1.Type", "fixed-response"),
        p("Actions.member.1.FixedResponseConfig.StatusCode", status),
        p("Actions.member.1.FixedResponseConfig.MessageBody", body),
    ];
    if let Some(content_type) = content_type {
        fields.push(p(
            "Actions.member.1.FixedResponseConfig.ContentType",
            content_type,
        ));
    }
    fields
}

/// One condition on a path pattern, which most rows carry so they match one path each.
fn path_pattern(pattern: &str) -> Vec<(String, String)> {
    vec![
        p("Conditions.member.1.Field", "path-pattern"),
        p("Conditions.member.1.Values.member.1", pattern),
    ]
}

/// Both fields of one request table row: what was sent, and what must come back.
struct Row {
    case: &'static str,
    method: &'static str,
    host: &'static str,
    target: &'static str,
    headers: &'static [(&'static str, &'static str)],
    status: u16,
    body: &'static str,
    /// The `Content-Type` the answer must carry, when the row asserts one.
    content_type: Option<&'static str>,
    /// The `Location` the answer must carry, for a redirect row.
    location: Option<&'static str>,
}

/// A row that asserts a status and a body alone.
fn answers(
    case: &'static str,
    method: &'static str,
    target: &'static str,
    status: u16,
    body: &'static str,
) -> Row {
    Row {
        case,
        method,
        host: "",
        target,
        headers: &[],
        status,
        body,
        content_type: None,
        location: None,
    }
}

/// Everything the fixture built, so each test drives the same deployment.
struct Deployment {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    lb_arn: String,
    dns_name: String,
    name: String,
}

/// The load balancer every row below is answered by: one HTTP:80 listener whose default
/// action is a fixed 404, and one rule per condition field and per action type.
async fn deployment() -> Deployment {
    let (router, engine) = emulator();
    let name = "dp-alb".to_string();
    let (lb_arn, dns_name) = create_load_balancer(&router, &name).await;
    let listener_arn = create_listener(&router, &lb_arn).await;
    let forwarded = create_http_target_group(&router, "dp-registered").await;
    let empty = create_http_target_group(&router, "dp-empty").await;

    // Two rules whose patterns overlap: the lower priority VALUE is evaluated first, so
    // `/a` is answered `a1` and the rule at 20 never runs.
    let mut fields = path_pattern("/a");
    fields.extend(fixed_response("200", "a1", None));
    add_rule(&router, "path /a at 10", &listener_arn, 10, &fields).await;
    let mut fields = path_pattern("/a");
    fields.extend(fixed_response("200", "b1", None));
    add_rule(&router, "path /a at 20", &listener_arn, 20, &fields).await;

    // A wildcard pattern, and the content type the action carries.
    let mut fields = path_pattern("/img/*");
    fields.extend(fixed_response(
        "200",
        "{\"ok\":true}",
        Some("application/json"),
    ));
    add_rule(&router, "path /img/*", &listener_arn, 30, &fields).await;

    // One rule per remaining condition field, each given through its own configuration
    // shape where it has one, so both spellings are exercised.
    let mut fields = vec![
        p("Conditions.member.1.Field", "host-header"),
        p(
            "Conditions.member.1.HostHeaderConfig.Values.member.1",
            "shop.example.com",
        ),
    ];
    fields.extend(fixed_response("200", "shop", None));
    add_rule(&router, "host-header", &listener_arn, 40, &fields).await;

    let mut fields = path_pattern("/m");
    fields.push(p("Conditions.member.2.Field", "http-request-method"));
    fields.push(p(
        "Conditions.member.2.HttpRequestMethodConfig.Values.member.1",
        "POST",
    ));
    fields.extend(fixed_response("201", "created", None));
    add_rule(&router, "http-request-method", &listener_arn, 50, &fields).await;

    let mut fields = vec![
        p("Conditions.member.1.Field", "http-header"),
        p(
            "Conditions.member.1.HttpHeaderConfig.HttpHeaderName",
            "X-Channel",
        ),
        p("Conditions.member.1.HttpHeaderConfig.Values.member.1", "mob*"),
    ];
    fields.extend(fixed_response("200", "mobile", None));
    add_rule(&router, "http-header", &listener_arn, 60, &fields).await;

    let mut fields = vec![
        p("Conditions.member.1.Field", "query-string"),
        p(
            "Conditions.member.1.QueryStringConfig.Values.member.1.Key",
            "ver",
        ),
        p(
            "Conditions.member.1.QueryStringConfig.Values.member.1.Value",
            "2",
        ),
    ];
    fields.extend(fixed_response("200", "v2", None));
    add_rule(&router, "query-string", &listener_arn, 70, &fields).await;

    // A condition this plane cannot evaluate, in front of an action it can: a `source-ip`
    // condition is about the CONNECTION's address, which a request served in-process does
    // not carry, so the rule must be passed over rather than matched on a header.
    let mut fields = path_pattern("/ip");
    fields.push(p("Conditions.member.2.Field", "source-ip"));
    fields.push(p(
        "Conditions.member.2.SourceIpConfig.Values.member.1",
        "10.0.0.0/8",
    ));
    fields.extend(fixed_response("200", "internal", None));
    add_rule(&router, "source-ip", &listener_arn, 80, &fields).await;

    // And one that matches on a regular expression, which this plane does not evaluate
    // either.
    let mut fields = vec![
        p("Conditions.member.1.Field", "path-pattern"),
        p(
            "Conditions.member.1.PathPatternConfig.RegexValues.member.1",
            "^/re/[0-9]+$",
        ),
    ];
    fields.extend(fixed_response("200", "regex", None));
    add_rule(&router, "regex values", &listener_arn, 85, &fields).await;

    // A redirect that changes the host and keeps the request's own path.
    let mut fields = path_pattern("/old");
    fields.push(p("Actions.member.1.Type", "redirect"));
    fields.push(p("Actions.member.1.RedirectConfig.StatusCode", "HTTP_301"));
    fields.push(p("Actions.member.1.RedirectConfig.Host", "new.example.com"));
    fields.push(p("Actions.member.1.RedirectConfig.Path", "/#{path}"));
    add_rule(&router, "redirect", &listener_arn, 90, &fields).await;

    // A forward to a group with a target, and one to a group with none.
    let mut fields = path_pattern("/tg");
    fields.push(p("Actions.member.1.Type", "forward"));
    fields.push(p("Actions.member.1.TargetGroupArn", &forwarded));
    add_rule(&router, "forward to a registered target", &listener_arn, 100, &fields).await;
    let mut fields = path_pattern("/empty");
    fields.push(p("Actions.member.1.Type", "forward"));
    fields.push(p("Actions.member.1.TargetGroupArn", &empty));
    add_rule(&router, "forward to an empty group", &listener_arn, 110, &fields).await;

    // An action this emulator cannot perform, in front of one it can: the request it
    // guards must not be served by falling through to the action behind it.
    let mut fields = path_pattern("/auth");
    fields.push(p("Actions.member.1.Type", "authenticate-oidc"));
    fields.push(p("Actions.member.1.Order", "1"));
    for (member, value) in [
        ("Issuer", "https://idp.example.com"),
        ("AuthorizationEndpoint", "https://idp.example.com/authorize"),
        ("TokenEndpoint", "https://idp.example.com/token"),
        ("UserInfoEndpoint", "https://idp.example.com/userinfo"),
        ("ClientId", "client-id"),
    ] {
        fields.push(p(
            &format!("Actions.member.1.AuthenticateOidcConfig.{member}"),
            value,
        ));
    }
    fields.push(p("Actions.member.2.Type", "fixed-response"));
    fields.push(p("Actions.member.2.Order", "2"));
    fields.push(p(
        "Actions.member.2.FixedResponseConfig.StatusCode",
        "200",
    ));
    fields.push(p(
        "Actions.member.2.FixedResponseConfig.MessageBody",
        "behind-the-idp",
    ));
    add_rule(&router, "authenticate-oidc", &listener_arn, 120, &fields).await;

    // One target in the forwarded group, which a load balancer now forwards to.
    let mut form = action("RegisterTargets");
    form.push(p("TargetGroupArn", &forwarded));
    form.push(p("Targets.member.1.Id", "i-11111111111111111"));
    let registered = call(&router, &form).await;
    assert_eq!(
        registered.status, 200,
        "case register a target: expected 200, got {} body {}",
        registered.status, registered.body
    );

    // The clock-owed moves, placed rather than slept for: the load balancer is active and
    // the registration is healthy.
    settle(&engine, &lb_arn, SystemTime::now() - Duration::from_secs(1));
    Deployment {
        router,
        engine,
        lb_arn,
        dns_name,
        name,
    }
}

/// Place the instant the load balancer's move to `active` and every registration's move to
/// `healthy` are owed at, so a case reads either side of it without sleeping.
pub(crate) fn settle(engine: &Arc<engine::InMemoryEngine>, lb_arn: &str, at: SystemTime) {
    let state = engine.state::<ElbState>();
    let mut store = state.lock();
    if let Some(record) = store.load_balancers.get_mut(lb_arn) {
        record.active_at = at;
    }
    for group in store.target_groups.values_mut() {
        for target in group.targets.values_mut() {
            target.healthy_at = at;
        }
    }
}

/// Every rule of the listener, as `DescribeRules` reports them: the read a data-plane
/// request must leave untouched.
///
/// The rule MEMBERS rather than the whole envelope, because each response carries its own
/// freshly minted `RequestId` — what two reads must agree on is the rules they report.
async fn described_rules(router: &MultiServiceRouter, lb_arn: &str) -> Vec<String> {
    let mut form = action("DescribeListeners");
    form.push(p("LoadBalancerArn", lb_arn));
    let listeners = call(router, &form).await;
    let listener_arn = listeners
        .text("ListenerArn")
        .unwrap_or_else(|| panic!("no ListenerArn in {}", listeners.body));
    let mut form = action("DescribeRules");
    form.push(p("ListenerArn", &listener_arn));
    let reply = call(router, &form).await;
    members(&reply.body, "Rules")
}

/// Every target of the group a rule forwards to, as `DescribeTargetHealth` reports them.
async fn described_health(router: &MultiServiceRouter, tg_arn: &str) -> Vec<String> {
    let mut form = action("DescribeTargetHealth");
    form.push(p("TargetGroupArn", tg_arn));
    let reply = call(router, &form).await;
    members(&reply.body, "TargetHealthDescriptions")
}

#[tokio::test]
async fn a_listener_answers_a_request_from_the_rule_its_conditions_match() {
    let deployed = deployment().await;
    let dns_name: &str = &deployed.dns_name;

    let mut rows = vec![
        // The overlapping patterns: the lower priority value wins, and the rule at 20 is
        // never reached.
        answers("the first matching rule by priority", "GET", "/a", 200, "a1"),
        // No rule's conditions are met, so the listener's default action answers.
        answers("no rule matches", "GET", "/b", 404, DEFAULT_BODY),
        answers("no rule matches a deeper path", "GET", "/a/b", 404, DEFAULT_BODY),
        // `*` matches zero or more characters.
        Row {
            content_type: Some("application/json"),
            ..answers("a wildcard pattern", "GET", "/img/cat.png", 200, "{\"ok\":true}")
        },
        Row {
            content_type: Some("application/json"),
            ..answers("a wildcard matching nothing", "GET", "/img/", 200, "{\"ok\":true}")
        },
        // The method condition, both ways.
        answers("the method the rule names", "POST", "/m", 201, "created"),
        answers("another method", "GET", "/m", 404, DEFAULT_BODY),
        // The default content type of a fixed response: "If the value is not specified,
        // the default value is text/plain."
        Row {
            content_type: Some("text/plain"),
            ..answers("the default content type", "GET", "/a", 200, "a1")
        },
        // A redirect's status and the Location its components build; the port is the
        // listener's 80, which the scheme implies and so is left off.
        Row {
            location: Some("http://new.example.com/old"),
            ..answers("a redirect", "GET", "/old", 301, "")
        },
        // A forward names targets with no server of this emulator's behind them.
        answers("a forward to a healthy target", "GET", "/tg", 502, ""),
        answers("a forward to a group with no target", "GET", "/empty", 503, ""),
        // An action this emulator cannot perform is not bypassed.
        answers("an action needing an identity provider", "GET", "/auth", 500, ""),
    ];
    // The conditions read off a header, the query string and the client address, each
    // with a matching and a non-matching value.
    rows.extend([
        Row {
            headers: &[("x-channel", "mobile")],
            ..answers("a header value the rule matches", "GET", "/h", 200, "mobile")
        },
        Row {
            headers: &[("x-channel", "web")],
            ..answers("a header value it does not", "GET", "/h", 404, DEFAULT_BODY)
        },
        Row {
            headers: &[],
            ..answers("the header absent", "GET", "/h", 404, DEFAULT_BODY)
        },
        answers("a query parameter the rule matches", "GET", "/q?ver=2", 200, "v2"),
        answers("a query parameter it does not", "GET", "/q?ver=3", 404, DEFAULT_BODY),
        // A condition this plane cannot evaluate leaves its rule unmatched rather than
        // matching it on a value the request itself supplied: a `source-ip` condition is
        // about the connection's address, which AWS documents the forwarded-for header
        // does NOT satisfy, and a regex condition needs an engine this plane has none of.
        Row {
            headers: &[("x-forwarded-for", "10.1.2.3")],
            ..answers("a source-ip rule and a forwarded client address", "GET", "/ip", 404, DEFAULT_BODY)
        },
        Row {
            headers: &[],
            ..answers("a source-ip rule and no forwarded address", "GET", "/ip", 404, DEFAULT_BODY)
        },
        answers("a rule matching on a regular expression", "GET", "/re/7", 404, DEFAULT_BODY),
    ]);

    for row in &rows {
        let host = if row.host.is_empty() {
            dns_name
        } else {
            row.host
        };
        let answered = send(&deployed.router, row.method, host, row.target, row.headers).await;
        assert_eq!(
            answered.status, row.status,
            "case {:?}: expected HTTP {}, got {} body {:?}",
            row.case, row.status, answered.status, answered.body
        );
        assert_eq!(
            answered.body, row.body,
            "case {:?}: expected body {:?}, got {:?}",
            row.case, row.body, answered.body
        );
        if let Some(content_type) = row.content_type {
            assert_eq!(
                answered.header("content-type"),
                Some(content_type),
                "case {:?}: expected content type {content_type:?}, got {:?}",
                row.case,
                answered.header("content-type")
            );
        }
        if let Some(location) = row.location {
            assert_eq!(
                answered.header("location"),
                Some(location),
                "case {:?}: expected Location {location:?}, got {:?}",
                row.case,
                answered.header("location")
            );
        }
    }

    // The host-header condition is about the host the CLIENT addressed, which on AWS is
    // an alias of the load balancer's own name. A client reaching this emulator carries
    // that host and names the load balancer in the path, so the condition is driven that
    // way — and the same request under the published DNS name matches no host-header rule.
    let prefixed = format!("/_aws/elasticloadbalancing/{dns_name}/z");
    let answered = send(
        &deployed.router,
        "GET",
        "shop.example.com",
        &prefixed,
        &[],
    )
    .await;
    assert_eq!(
        (answered.status, answered.body.as_str()),
        (200, "shop"),
        "case the host the rule names: expected the host-header rule's answer, got {} {:?}",
        answered.status,
        answered.body
    );
    let answered = send(&deployed.router, "GET", dns_name, "/z", &[]).await;
    assert_eq!(
        (answered.status, answered.body.as_str()),
        (404, DEFAULT_BODY),
        "case another host: the host-header rule does not match it, got {} {:?}",
        answered.status,
        answered.body
    );
}

#[tokio::test]
async fn a_load_balancer_is_reached_by_every_addressing_a_client_can_use() {
    let deployed = deployment().await;
    // Each addressing resolves the SAME load balancer, so each answers the rule at
    // priority 10 for `/a`.
    let prefixed = format!("/_aws/elasticloadbalancing/{}/a", deployed.dns_name);
    let by_name = format!("/_aws/elasticloadbalancing/{}/a", deployed.name);
    let path_style = format!("/{}/a", deployed.name);
    let rows: Vec<(&str, &str, &str)> = vec![
        ("the published DNS name", deployed.dns_name.as_str(), "/a"),
        ("the reserved prefix and the DNS name", EMULATOR_HOST, prefixed.as_str()),
        ("the reserved prefix and the name", EMULATOR_HOST, by_name.as_str()),
        ("the name as the first path segment", EMULATOR_HOST, path_style.as_str()),
        ("the request path alone", EMULATOR_HOST, "/a"),
    ];
    for (case, host, target) in &rows {
        let answered = send(&deployed.router, "GET", host, target, &[]).await;
        assert_eq!(
            (answered.status, answered.body.as_str()),
            (200, "a1"),
            "case {case:?}: expected the rule's own answer, got {} {:?}",
            answered.status,
            answered.body
        );
    }

    // A request that addresses ANOTHER service's endpoint or another data plane's reserved
    // namespace, presents the credentials of an AWS API call (signed or presigned), or
    // carries a path no rule of this load balancer is written for, is not this load
    // balancer's traffic and is not answered as such — which is what leaves one claimant
    // per request when another plane reads a host-independent addressing of its own.
    struct Unclaimed {
        case: &'static str,
        host: &'static str,
        target: &'static str,
        headers: &'static [(&'static str, &'static str)],
    }
    let unclaimed: &[Unclaimed] = &[
        Unclaimed {
            case: "another service's endpoint",
            host: "s3.us-west-2.amazonaws.com",
            target: "/a",
            headers: &[],
        },
        Unclaimed {
            case: "another data plane's reserved namespace",
            host: EMULATOR_HOST,
            target: "/_aws/execute-api/abc123/prod/a",
            headers: &[],
        },
        Unclaimed {
            case: "a path no rule of this load balancer is written for",
            host: EMULATOR_HOST,
            target: "/prod/orders",
            headers: &[],
        },
        Unclaimed {
            case: "a signed API call on the emulator's endpoint",
            host: EMULATOR_HOST,
            target: "/a",
            headers: &[(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/us-west-2/s3/aws4_request, \
                 SignedHeaders=host, Signature=0123456789abcdef",
            )],
        },
        Unclaimed {
            case: "an unsigned API call carrying only the signing date",
            host: EMULATOR_HOST,
            target: "/a",
            headers: &[("x-amz-date", "20260101T000000Z")],
        },
    ];    for row in unclaimed {
        let answered = send(&deployed.router, "GET", row.host, row.target, row.headers).await;
        assert_ne!(
            answered.body, "a1",
            "case {:?}: the request is not this load balancer's traffic, got {} {:?}",
            row.case, answered.status, answered.body
        );
    }

    // A presigned call is an AWS API call too, however little else it presents.
    let answered = send(
        &deployed.router,
        "GET",
        EMULATOR_HOST,
        "/a?X-Amz-Credential=AKIDEXAMPLE%2F20260101%2Fus-west-2%2Fs3%2Faws4_request",
        &[],
    )
    .await;
    assert_ne!(
        answered.body, "a1",
        "case a presigned API call: the request is not this load balancer's traffic, got {} {:?}",
        answered.status, answered.body
    );

    // An `Authorization` that is NOT SigV4 is ordinary traffic through a load balancer —
    // the credential an application behind it reads — so it is still answered by the rule.
    for credential in ["Basic dXNlcjpwYXNz", "Bearer a-token"] {
        let answered = send(
            &deployed.router,
            "GET",
            EMULATOR_HOST,
            "/a",
            &[("authorization", credential)],
        )
        .await;
        assert_eq!(
            (answered.status, answered.body.as_str()),
            (200, "a1"),
            "case {credential:?}: traffic carrying a non-SigV4 credential is the load \
             balancer's to answer, got {} {:?}",
            answered.status,
            answered.body
        );
    }
}

#[tokio::test]
async fn a_load_balancer_that_is_still_provisioning_serves_no_traffic_and_reads_change_nothing() {
    let deployed = deployment().await;

    // Before: the rules a read reports, and the health a read reports.
    let target_group = target_group_arn(&deployed);
    let rules_before = described_rules(&deployed.router, &deployed.lb_arn).await;
    let health_before = described_health(&deployed.router, &target_group).await;

    // A burst of data-plane requests, each answered with what its own rule says, so the
    // burst cannot pass on a wrong answer.
    let burst: &[(&str, u16, &str)] = &[
        ("/a", 200, "a1"),
        ("/b", 404, DEFAULT_BODY),
        ("/img/x", 200, "{\"ok\":true}"),
        ("/old", 301, ""),
        ("/tg", 502, ""),
        ("/empty", 503, ""),
        ("/auth", 500, ""),
    ];
    for (target, status, body) in burst {
        let answered = send(&deployed.router, "GET", &deployed.dns_name, target, &[]).await;
        assert_eq!(
            (answered.status, answered.body.as_str()),
            (*status, *body),
            "case {target:?}: expected HTTP {status} {body:?}, got {} {:?}",
            answered.status,
            answered.body
        );
    }

    // Serving traffic moves no status, consumes no quota and bumps no counter: both reads
    // report exactly what they did before.
    let rules_after = described_rules(&deployed.router, &deployed.lb_arn).await;
    assert_eq!(
        rules_after, rules_before,
        "case rules after serving: a read must report what it did before, got {rules_after:?}"
    );
    let health_after = described_health(&deployed.router, &target_group).await;
    assert_eq!(
        health_after, health_before,
        "case target health after serving: a read must report what it did before, got \
         {health_after:?}"
    );

    // A load balancer the clock still owes its move out of `provisioning` is not serving
    // traffic on the name it published yet.
    settle(
        &deployed.engine,
        &deployed.lb_arn,
        SystemTime::now() + Duration::from_secs(3600),
    );
    let answered = send(&deployed.router, "GET", &deployed.dns_name, "/a", &[]).await;
    assert_eq!(
        (answered.status, answered.body.as_str()),
        (503, ""),
        "case provisioning: a load balancer still coming up serves nothing, got {} {:?}",
        answered.status,
        answered.body
    );

    // And once the move is due, the same request is answered from the same records.
    settle(
        &deployed.engine,
        &deployed.lb_arn,
        SystemTime::now() - Duration::from_secs(1),
    );
    let answered = send(&deployed.router, "GET", &deployed.dns_name, "/a", &[]).await;
    assert_eq!(
        (answered.status, answered.body.as_str()),
        (200, "a1"),
        "case active: the rule answers once the load balancer is active, got {} {:?}",
        answered.status,
        answered.body
    );
}

/// The ARN of the target group a rule forwards to, read out of the store the control plane
/// wrote it to.
fn target_group_arn(deployed: &Deployment) -> String {
    let state = deployed.engine.state::<ElbState>();
    let store = state.lock();
    store
        .name_to_tg_arn
        .get("dp-registered")
        .cloned()
        .expect("the fixture created the group")
}
