// SPDX-License-Identifier: CC0-1.0

//! Listeners, the health check and the attributes of a Classic load balancer, end to end
//! over the wire.
//!
//! Every request goes through the awsQuery protocol on the multi-service router (see
//! `super::wire`), so what is asserted is the emulator's HTTP behaviour.
//!
//! The rules under test, each over a TABLE of inputs rather than one recorded request:
//!
//! * `CreateLoadBalancerListeners` adds a listener the record does not configure yet,
//!   accepts a repeat of one it already configures identically, and refuses one that
//!   would change a configured port (`DuplicateListener`). `DeleteLoadBalancerListeners`
//!   removes the named ports and succeeds whether or not a listener was configured there.
//! * `ConfigureHealthCheck` writes the requested check onto the record, answers what the
//!   record now holds, and refuses a check the model's documented rules rule out.
//! * `DescribeLoadBalancerAttributes` reports the service defaults for a load balancer
//!   nobody has modified; `ModifyLoadBalancerAttributes` writes the attributes the
//!   request supplies, echoes exactly those back, and leaves the others as they were.
//!
//! Every mutation is asserted on what a LATER read reports, never on what the mutation
//! answered alone.

use super::wire::{
    call, emulator, kid, kid_text, members, param, parse, Element, Wire,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-east-1";

/// One listener as [`described_listeners`] reports it: its load balancer port, its two
/// protocols, its instance port and its certificate.
type ReportedListener = (i32, String, String, i32, Option<String>);

/// One row of a refusal table that spans two operations: the case, the action, the form
/// fields, the expected HTTP status, and the expected awsQuery `<Code>` (`None` where the
/// refusal is the framework's own, for a constraint the model declares).
type OperationRefusalRow = (
    &'static str,
    &'static str,
    Vec<(String, String)>,
    u16,
    Option<&'static str>,
);

/// One row of a refusal table for a single operation: as [`OperationRefusalRow`] without
/// the action.
type RefusalRow = (
    &'static str,
    Vec<(String, String)>,
    u16,
    Option<&'static str>,
);

/// One listener, as a request carries it and as a describe reports it back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Listener {
    protocol: &'static str,
    load_balancer_port: i32,
    /// `None` exercises the documented default: the front-end protocol.
    instance_protocol: Option<&'static str>,
    instance_port: i32,
    ssl_certificate_id: Option<&'static str>,
}

impl Listener {
    /// `Listeners.member.<slot>.*` form fields for this listener.
    fn params(&self, slot: usize) -> Vec<(String, String)> {
        let mut params = vec![
            param(
                &format!("Listeners.member.{slot}.Protocol"),
                self.protocol,
            ),
            param(
                &format!("Listeners.member.{slot}.LoadBalancerPort"),
                &self.load_balancer_port.to_string(),
            ),
            param(
                &format!("Listeners.member.{slot}.InstancePort"),
                &self.instance_port.to_string(),
            ),
        ];
        if let Some(instance_protocol) = self.instance_protocol {
            params.push(param(
                &format!("Listeners.member.{slot}.InstanceProtocol"),
                instance_protocol,
            ));
        }
        if let Some(certificate) = self.ssl_certificate_id {
            params.push(param(
                &format!("Listeners.member.{slot}.SSLCertificateId"),
                certificate,
            ));
        }
        params
    }

    /// The `InstanceProtocol` the record must report: the one the request named, else the
    /// front-end `Protocol` (`Listener$InstanceProtocol`'s documented default).
    fn reported_instance_protocol(&self) -> &'static str {
        self.instance_protocol.unwrap_or(self.protocol)
    }
}

/// `Listeners.member.*` form fields for a whole list.
fn listener_params(listeners: &[Listener]) -> Vec<(String, String)> {
    listeners
        .iter()
        .enumerate()
        .flat_map(|(index, listener)| listener.params(index + 1))
        .collect()
}

/// Create a load balancer named `name` with `listeners`, and fail the test if the create
/// does not answer 200.
async fn create(router: &MultiServiceRouter, name: &str, listeners: &[Listener]) {
    let mut params = vec![
        param("LoadBalancerName", name),
        param("AvailabilityZones.member.1", "us-east-1a"),
    ];
    params.extend(listener_params(listeners));
    let created = call(router, REGION, "CreateLoadBalancer", &params).await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: CreateLoadBalancer must answer 200, got {} with {}",
        created.status, created.body
    );
}

/// The `ListenerDescriptions` of `name`, keyed by `LoadBalancerPort`, as
/// `DescribeLoadBalancers` reports them.
async fn described_listeners(router: &MultiServiceRouter, name: &str) -> Vec<ReportedListener> {
    let described = call(
        router,
        REGION,
        "DescribeLoadBalancers",
        &[param("LoadBalancerNames.member.1", name)],
    )
    .await;
    let doc = parse(&described.body);
    let result = described.result(&doc);
    let descriptions = members(result, "LoadBalancerDescriptions");
    let description = descriptions
        .first()
        .unwrap_or_else(|| panic!("case {name:?}: no load balancer in {}", described.body));
    let mut reported: Vec<ReportedListener> = members(description, "ListenerDescriptions")
        .into_iter()
        .map(|wrapper| {
            let listener = kid(wrapper, "Listener")
                .unwrap_or_else(|| panic!("case {name:?}: no Listener in {}", described.body));
            (
                number(listener, "LoadBalancerPort"),
                kid_text(listener, "Protocol").unwrap_or_default(),
                kid_text(listener, "InstanceProtocol").unwrap_or_default(),
                number(listener, "InstancePort"),
                kid_text(listener, "SSLCertificateId"),
            )
        })
        .collect();
    reported.sort_by_key(|reported| reported.0);
    reported
}

/// The integer text of the child element named `tag`.
fn number(node: &Element, tag: &str) -> i32 {
    kid_text(node, tag)
        .unwrap_or_default()
        .parse()
        .unwrap_or_else(|_| panic!("{tag} must be an integer, got {:?}", kid_text(node, tag)))
}

/// A listener as [`described_listeners`] reports it.
fn expected(listener: &Listener) -> ReportedListener {
    (
        listener.load_balancer_port,
        listener.protocol.to_string(),
        listener.reported_instance_protocol().to_string(),
        listener.instance_port,
        listener.ssl_certificate_id.map(str::to_string),
    )
}

/// The listener set of a load balancer is driven by the two listener operations, and
/// every assertion is made on what `DescribeLoadBalancers` reports afterwards.
///
/// The rows differ along every axis the rule generalises over: the transport family
/// (HTTP/HTTPS against TCP/SSL), an explicit versus a defaulted `InstanceProtocol`, a
/// listener with and without an `SSLCertificateId`, one added listener against three in
/// one call, and the ports they claim.
#[tokio::test]
async fn listeners_are_added_repeated_refused_and_removed() {
    /// The listener every load balancer below is created with, so the added ones are the
    /// only difference between the rows.
    const BASE: Listener = Listener {
        protocol: "HTTP",
        load_balancer_port: 80,
        instance_protocol: None,
        instance_port: 8080,
        ssl_certificate_id: None,
    };

    // (case, the listeners added in one call)
    let rows: [(&str, &[Listener]); 5] = [
        (
            "one HTTPS listener with a certificate",
            &[Listener {
                protocol: "HTTPS",
                load_balancer_port: 443,
                instance_protocol: Some("HTTP"),
                instance_port: 8080,
                ssl_certificate_id: Some("arn:aws:iam::000000000000:server-certificate/listener"),
            }],
        ),
        (
            "one TCP listener, InstanceProtocol defaulted",
            &[Listener {
                protocol: "TCP",
                load_balancer_port: 9000,
                instance_protocol: None,
                instance_port: 9000,
                ssl_certificate_id: None,
            }],
        ),
        (
            "one SSL listener over a TCP back end",
            &[Listener {
                protocol: "SSL",
                load_balancer_port: 6443,
                instance_protocol: Some("TCP"),
                instance_port: 6443,
                ssl_certificate_id: Some("arn:aws:iam::000000000000:server-certificate/ssl"),
            }],
        ),
        (
            "three listeners in one call",
            &[
                Listener {
                    protocol: "HTTP",
                    load_balancer_port: 8081,
                    instance_protocol: None,
                    instance_port: 18081,
                    ssl_certificate_id: None,
                },
                Listener {
                    protocol: "TCP",
                    load_balancer_port: 1,
                    instance_protocol: None,
                    instance_port: 1,
                    ssl_certificate_id: None,
                },
                Listener {
                    protocol: "HTTPS",
                    load_balancer_port: 65535,
                    instance_protocol: Some("HTTPS"),
                    instance_port: 65535,
                    ssl_certificate_id: Some("arn:aws:iam::000000000000:server-certificate/high"),
                },
            ],
        ),
        (
            // Lower-cased on the wire: the protocol set is a documented value list, not a
            // case-sensitive token, and the record reports the canonical spelling.
            "a lower-cased protocol",
            &[Listener {
                protocol: "http",
                load_balancer_port: 8888,
                instance_protocol: Some("http"),
                instance_port: 18888,
                ssl_certificate_id: None,
            }],
        ),
    ];

    let router = emulator();
    for (index, (case, added)) in rows.into_iter().enumerate() {
        let name = format!("listeners-{index}");
        create(&router, &name, &[BASE]).await;

        let mut params = vec![param("LoadBalancerName", &name)];
        params.extend(listener_params(added));
        let created = call(&router, REGION, "CreateLoadBalancerListeners", &params).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateLoadBalancerListeners must answer 200, got {} with {}",
            created.status, created.body
        );
        // An awsQuery success envelope, not the router's JSON refusal.
        assert!(
            created
                .body
                .contains("<CreateLoadBalancerListenersResponse"),
            "case {case:?}: expected a CreateLoadBalancerListenersResponse envelope, got {}",
            created.body
        );

        let mut want: Vec<_> = std::iter::once(&BASE)
            .chain(added.iter())
            .map(expected)
            .collect();
        want.sort_by_key(|listener| listener.0);
        // The protocols are reported in the canonical upper-case spelling.
        let want: Vec<_> = want
            .into_iter()
            .map(|(port, protocol, instance_protocol, instance_port, certificate)| {
                (
                    port,
                    protocol.to_ascii_uppercase(),
                    instance_protocol.to_ascii_uppercase(),
                    instance_port,
                    certificate,
                )
            })
            .collect();
        let reported = described_listeners(&router, &name).await;
        assert_eq!(
            reported, want,
            "case {case:?}: DescribeLoadBalancers must report the added listeners, got \
             {reported:?}"
        );

        // The SAME call again: "If a listener with the specified port does not already
        // exist, it is created; otherwise, the properties of the new listener must match
        // the properties of the existing listener" — so a repeat is accepted and adds
        // nothing.
        let repeated = call(&router, REGION, "CreateLoadBalancerListeners", &params).await;
        assert_eq!(
            repeated.status, 200,
            "case {case:?}: repeating an identical CreateLoadBalancerListeners must \
             answer 200, got {} with {}",
            repeated.status, repeated.body
        );
        let after_repeat = described_listeners(&router, &name).await;
        assert_eq!(
            after_repeat, want,
            "case {case:?}: a repeated create must leave the listener set alone, got \
             {after_repeat:?}"
        );

        // The same port with a DIFFERENT configuration is the modelled conflict, and it
        // changes nothing.
        let conflicting = Listener {
            instance_port: added[0].instance_port + 1,
            ..added[0]
        };
        let mut conflict_params = vec![param("LoadBalancerName", &name)];
        conflict_params.extend(listener_params(&[conflicting]));
        let refused = call(
            &router,
            REGION,
            "CreateLoadBalancerListeners",
            &conflict_params,
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "DuplicateListener"),
            "case {case:?}: a different configuration on a configured port must be \
             refused DuplicateListener, got {} with {}",
            refused.status,
            refused.body
        );
        let after_conflict = described_listeners(&router, &name).await;
        assert_eq!(
            after_conflict, want,
            "case {case:?}: a refused create must leave the listener set alone, got \
             {after_conflict:?}"
        );

        // Delete the added ports, twice: the second delete is the documented no-op, so
        // both answer 200 and the base listener survives both.
        let ports: Vec<(String, String)> = added
            .iter()
            .enumerate()
            .map(|(index, listener)| {
                param(
                    &format!("LoadBalancerPorts.member.{}", index + 1),
                    &listener.load_balancer_port.to_string(),
                )
            })
            .collect();
        let mut delete_params = vec![param("LoadBalancerName", &name)];
        delete_params.extend(ports);
        for attempt in 1..=2 {
            let deleted = call(
                &router,
                REGION,
                "DeleteLoadBalancerListeners",
                &delete_params,
            )
            .await;
            assert_eq!(
                deleted.status, 200,
                "case {case:?}: DeleteLoadBalancerListeners attempt {attempt} must answer \
                 200, got {} with {}",
                deleted.status, deleted.body
            );
            let remaining = described_listeners(&router, &name).await;
            assert_eq!(
                remaining,
                vec![expected(&BASE)],
                "case {case:?}: attempt {attempt} must leave only the base listener, got \
                 {remaining:?}"
            );
        }
    }
}

/// A listener operation names a load balancer no record holds, or a configuration the
/// model's rules rule out. The rows span both listener operations and every rule.
#[tokio::test]
async fn listener_requests_the_rules_rule_out_are_refused() {
    let router = emulator();
    create(
        &router,
        "listener-refusals",
        &[Listener {
            protocol: "HTTP",
            load_balancer_port: 80,
            instance_protocol: None,
            instance_port: 8080,
            ssl_certificate_id: None,
        }],
    )
    .await;

    // (case, action, form fields, expected status, expected awsQuery code — `None` where
    // the code is the framework's own boundary wording rather than a modelled one)
    let rows: [OperationRefusalRow; 6] = [
        (
            "a create for a load balancer no record holds",
            "CreateLoadBalancerListeners",
            {
                let mut params = vec![param("LoadBalancerName", "no-such-load-balancer")];
                params.extend(listener_params(&[Listener {
                    protocol: "HTTP",
                    load_balancer_port: 80,
                    instance_protocol: None,
                    instance_port: 8080,
                    ssl_certificate_id: None,
                }]));
                params
            },
            400,
            Some("LoadBalancerNotFound"),
        ),
        (
            "a delete for a load balancer no record holds",
            "DeleteLoadBalancerListeners",
            vec![
                param("LoadBalancerName", "no-such-load-balancer"),
                param("LoadBalancerPorts.member.1", "80"),
            ],
            400,
            Some("LoadBalancerNotFound"),
        ),
        (
            // `Protocol` is documented as "HTTP, HTTPS, TCP, or SSL"; anything else is
            // the protocol error the operation declares.
            "a protocol outside the documented set",
            "CreateLoadBalancerListeners",
            {
                let mut params = vec![param("LoadBalancerName", "listener-refusals")];
                params.extend(listener_params(&[Listener {
                    protocol: "UDP",
                    load_balancer_port: 7000,
                    instance_protocol: None,
                    instance_port: 7000,
                    ssl_certificate_id: None,
                }]));
                params
            },
            400,
            Some("UnsupportedProtocol"),
        ),
        (
            // "If the front-end protocol is HTTP or HTTPS, the back-end protocol must be
            // HTTP or HTTPS."
            "a back-end protocol from the other transport family",
            "CreateLoadBalancerListeners",
            {
                let mut params = vec![param("LoadBalancerName", "listener-refusals")];
                params.extend(listener_params(&[Listener {
                    protocol: "HTTP",
                    load_balancer_port: 7001,
                    instance_protocol: Some("TCP"),
                    instance_port: 7001,
                    ssl_certificate_id: None,
                }]));
                params
            },
            409,
            Some("InvalidConfigurationRequest"),
        ),
        (
            "one load balancer port claimed twice in one request",
            "CreateLoadBalancerListeners",
            {
                let mut params = vec![param("LoadBalancerName", "listener-refusals")];
                params.extend(listener_params(&[
                    Listener {
                        protocol: "HTTP",
                        load_balancer_port: 7002,
                        instance_protocol: None,
                        instance_port: 7002,
                        ssl_certificate_id: None,
                    },
                    Listener {
                        protocol: "HTTP",
                        load_balancer_port: 7002,
                        instance_protocol: None,
                        instance_port: 7003,
                        ssl_certificate_id: None,
                    },
                ]));
                params
            },
            409,
            Some("InvalidConfigurationRequest"),
        ),
        (
            // `InstancePort` is modelled `@range(min: 1, max: 65535)`, so the boundary
            // validator refuses it before the handler runs.
            "an instance port past the modelled maximum",
            "CreateLoadBalancerListeners",
            {
                let mut params = vec![param("LoadBalancerName", "listener-refusals")];
                params.extend(listener_params(&[Listener {
                    protocol: "HTTP",
                    load_balancer_port: 7004,
                    instance_protocol: None,
                    instance_port: 65536,
                    ssl_certificate_id: None,
                }]));
                params
            },
            400,
            None,
        ),
    ];

    for (case, action, params, status, code) in rows {
        let refused = call(&router, REGION, action, &params).await;
        assert_eq!(
            refused.status, status,
            "case {case:?}: expected HTTP {status}, got {} with {}",
            refused.status, refused.body
        );
        assert!(
            refused.body.contains("<ErrorResponse"),
            "case {case:?}: an awsQuery client must get an <ErrorResponse> envelope, got {}",
            refused.body
        );
        if let Some(code) = code {
            assert_eq!(
                refused.error_code(),
                code,
                "case {case:?}: expected the error code {code}, got {}",
                refused.body
            );
        }
    }

    // The refused requests above stored nothing: the load balancer still has exactly the
    // listener it was created with.
    let reported = described_listeners(&router, "listener-refusals").await;
    assert_eq!(
        reported.len(),
        1,
        "a refused listener request must store nothing, got {reported:?}"
    );
}

/// One health check, as a request carries it.
#[derive(Debug, Clone, Copy)]
struct HealthCheck {
    target: &'static str,
    interval: i32,
    timeout: i32,
    unhealthy_threshold: i32,
    healthy_threshold: i32,
}

impl HealthCheck {
    fn params(&self) -> Vec<(String, String)> {
        vec![
            param("HealthCheck.Target", self.target),
            param("HealthCheck.Interval", &self.interval.to_string()),
            param("HealthCheck.Timeout", &self.timeout.to_string()),
            param(
                "HealthCheck.UnhealthyThreshold",
                &self.unhealthy_threshold.to_string(),
            ),
            param(
                "HealthCheck.HealthyThreshold",
                &self.healthy_threshold.to_string(),
            ),
        ]
    }

    /// Assert that `element` is this health check.
    fn assert_reported(&self, case: &str, where_: &str, element: &Element, body: &str) {
        for (member, expected) in [
            ("Target", self.target.to_string()),
            ("Interval", self.interval.to_string()),
            ("Timeout", self.timeout.to_string()),
            ("UnhealthyThreshold", self.unhealthy_threshold.to_string()),
            ("HealthyThreshold", self.healthy_threshold.to_string()),
        ] {
            assert_eq!(
                kid_text(element, member),
                Some(expected.clone()),
                "case {case:?}: {where_} must report HealthCheck.{member} = {expected}, \
                 got {body}"
            );
        }
    }
}

/// `ConfigureHealthCheck` writes the requested check onto the record and every later read
/// reports it. The rows span the target forms the model documents and each published
/// bound of the four numeric members.
#[tokio::test]
async fn the_health_check_round_trips_through_the_stored_record() {
    // The rows' targets cover all four documented protocols, with the HTTP forms carrying
    // the ping path the documentation requires; the numbers cover the published bounds
    // (`Interval` 5..300, `Timeout` 2..60 and less than `Interval`, both thresholds
    // 2..10).
    let rows: [(&str, HealthCheck); 5] = [
        (
            "an HTTP check at the lower bounds",
            HealthCheck {
                target: "HTTP:80/index.html",
                interval: 5,
                timeout: 2,
                unhealthy_threshold: 2,
                healthy_threshold: 2,
            },
        ),
        (
            "an HTTPS check at the upper bounds",
            HealthCheck {
                target: "HTTPS:443/weather/us/wa/seattle",
                interval: 300,
                timeout: 60,
                unhealthy_threshold: 10,
                healthy_threshold: 10,
            },
        ),
        (
            "a TCP check on the highest port",
            HealthCheck {
                target: "TCP:65535",
                interval: 30,
                timeout: 5,
                unhealthy_threshold: 3,
                healthy_threshold: 4,
            },
        ),
        (
            "an SSL check on the lowest port",
            HealthCheck {
                target: "SSL:1",
                interval: 10,
                timeout: 9,
                unhealthy_threshold: 5,
                healthy_threshold: 6,
            },
        ),
        (
            "an HTTP check on a deep ping path",
            HealthCheck {
                target: "HTTP:8080/a/b/c/d/e/health?probe=1",
                interval: 299,
                timeout: 59,
                unhealthy_threshold: 9,
                healthy_threshold: 2,
            },
        ),
    ];

    let router = emulator();
    for (index, (case, health_check)) in rows.into_iter().enumerate() {
        let name = format!("health-check-{index}");
        create(
            &router,
            &name,
            &[Listener {
                protocol: "HTTP",
                load_balancer_port: 80,
                instance_protocol: None,
                instance_port: 8080,
                ssl_certificate_id: None,
            }],
        )
        .await;

        let mut params = vec![param("LoadBalancerName", &name)];
        params.extend(health_check.params());
        let configured = call(&router, REGION, "ConfigureHealthCheck", &params).await;
        assert_eq!(
            configured.status, 200,
            "case {case:?}: ConfigureHealthCheck must answer 200, got {} with {}",
            configured.status, configured.body
        );
        let doc = parse(&configured.body);
        let result = configured.result(&doc);
        let answered = kid(result, "HealthCheck").unwrap_or_else(|| {
            panic!("case {case:?}: no HealthCheck in {}", configured.body)
        });
        health_check.assert_reported(case, "ConfigureHealthCheck", answered, &configured.body);

        // The read back: the same configuration, from the record the configure wrote.
        let described = call(
            &router,
            REGION,
            "DescribeLoadBalancers",
            &[param("LoadBalancerNames.member.1", &name)],
        )
        .await;
        let doc = parse(&described.body);
        let result = described.result(&doc);
        let description = *members(result, "LoadBalancerDescriptions")
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: no load balancer in {}", described.body));
        let stored = kid(description, "HealthCheck")
            .unwrap_or_else(|| panic!("case {case:?}: no HealthCheck in {}", described.body));
        health_check.assert_reported(case, "DescribeLoadBalancers", stored, &described.body);
    }
}

/// A health check the model's rules rule out is refused, and the stored check is left
/// alone. Each numeric row is one past a published bound; each target row breaks one
/// clause of the documented target grammar.
#[tokio::test]
async fn health_checks_the_rules_rule_out_are_refused() {
    const VALID: HealthCheck = HealthCheck {
        target: "HTTP:80/ping",
        interval: 30,
        timeout: 5,
        unhealthy_threshold: 2,
        healthy_threshold: 2,
    };

    let router = emulator();
    create(
        &router,
        "health-check-refusals",
        &[Listener {
            protocol: "HTTP",
            load_balancer_port: 80,
            instance_protocol: None,
            instance_port: 8080,
            ssl_certificate_id: None,
        }],
    )
    .await;
    // The check the refusals below must not disturb.
    let mut params = vec![param("LoadBalancerName", "health-check-refusals")];
    params.extend(VALID.params());
    let configured = call(&router, REGION, "ConfigureHealthCheck", &params).await;
    assert_eq!(
        configured.status, 200,
        "the baseline ConfigureHealthCheck must answer 200, got {} with {}",
        configured.status, configured.body
    );

    // (case, the check, expected status, expected code — `None` where the refusal is the
    // framework's own, for a constraint the model declares)
    let rows: [(&str, HealthCheck, u16, Option<&str>); 9] = [
        (
            "an interval one below the modelled minimum",
            HealthCheck { interval: 4, ..VALID },
            400,
            None,
        ),
        (
            "an interval one above the modelled maximum",
            HealthCheck { interval: 301, ..VALID },
            400,
            None,
        ),
        (
            "a timeout one below the modelled minimum",
            HealthCheck { timeout: 1, ..VALID },
            400,
            None,
        ),
        (
            "a timeout one above the modelled maximum",
            HealthCheck {
                interval: 300,
                timeout: 61,
                ..VALID
            },
            400,
            None,
        ),
        (
            "a threshold one above the modelled maximum",
            HealthCheck {
                healthy_threshold: 11,
                ..VALID
            },
            400,
            None,
        ),
        (
            // "This value must be less than the Interval value" — documented, not
            // modelled, so this handler is what refuses it.
            "a timeout equal to the interval",
            HealthCheck {
                interval: 10,
                timeout: 10,
                ..VALID
            },
            400,
            Some("ValidationException"),
        ),
        (
            "a target naming no protocol",
            HealthCheck {
                target: "80/ping",
                ..VALID
            },
            400,
            Some("ValidationException"),
        ),
        (
            // "For HTTP/HTTPS, you must include a ping path in the string."
            "an HTTP target with no ping path",
            HealthCheck {
                target: "HTTP:80",
                ..VALID
            },
            400,
            Some("ValidationException"),
        ),
        (
            // "The range of valid ports is one (1) through 65535."
            "a target port past the documented maximum",
            HealthCheck {
                target: "TCP:65536",
                ..VALID
            },
            400,
            Some("ValidationException"),
        ),
    ];

    for (case, health_check, status, code) in rows {
        let mut params = vec![param("LoadBalancerName", "health-check-refusals")];
        params.extend(health_check.params());
        let refused = call(&router, REGION, "ConfigureHealthCheck", &params).await;
        assert_eq!(
            refused.status, status,
            "case {case:?}: expected HTTP {status}, got {} with {}",
            refused.status, refused.body
        );
        if let Some(code) = code {
            assert_eq!(
                refused.error_code(),
                code,
                "case {case:?}: expected the error code {code}, got {}",
                refused.body
            );
        }
    }

    // A refused configure wrote nothing: the baseline check is still the stored one.
    let described = call(
        &router,
        REGION,
        "DescribeLoadBalancers",
        &[param("LoadBalancerNames.member.1", "health-check-refusals")],
    )
    .await;
    let doc = parse(&described.body);
    let result = described.result(&doc);
    let description = *members(result, "LoadBalancerDescriptions")
        .first()
        .expect("the load balancer must still be described");
    let stored = kid(description, "HealthCheck").expect("a stored HealthCheck");
    VALID.assert_reported(
        "a refused configure",
        "DescribeLoadBalancers",
        stored,
        &described.body,
    );

    // A configure for a load balancer no record holds is the operation's one modelled
    // failure.
    let mut params = vec![param("LoadBalancerName", "no-such-load-balancer")];
    params.extend(VALID.params());
    let missing = call(&router, REGION, "ConfigureHealthCheck", &params).await;
    assert_eq!(
        (missing.status, missing.error_code().as_str()),
        (400, "LoadBalancerNotFound"),
        "a configure for a missing load balancer must be refused LoadBalancerNotFound, \
         got {} with {}",
        missing.status,
        missing.body
    );
}

/// The attributes `DescribeLoadBalancerAttributes` reports, flattened to the
/// `(path, value)` pairs this suite asserts on.
fn reported_attributes(wire: &Wire) -> Vec<(String, String)> {
    let doc = parse(&wire.body);
    let result = wire.result(&doc);
    let attributes = kid(result, "LoadBalancerAttributes")
        .unwrap_or_else(|| panic!("no LoadBalancerAttributes in {}", wire.body));
    let mut pairs: Vec<(String, String)> = Vec::new();
    for (group, member) in [
        ("CrossZoneLoadBalancing", "Enabled"),
        ("AccessLog", "Enabled"),
        ("AccessLog", "EmitInterval"),
        ("AccessLog", "S3BucketName"),
        ("AccessLog", "S3BucketPrefix"),
        ("ConnectionDraining", "Enabled"),
        ("ConnectionDraining", "Timeout"),
        ("ConnectionSettings", "IdleTimeout"),
    ] {
        if let Some(value) = kid(attributes, group).and_then(|group| kid_text(group, member)) {
            pairs.push((format!("{group}.{member}"), value));
        }
    }
    for additional in members(attributes, "AdditionalAttributes") {
        let key = kid_text(additional, "Key").unwrap_or_default();
        let value = kid_text(additional, "Value").unwrap_or_default();
        pairs.push((format!("AdditionalAttributes.{key}"), value));
    }
    pairs.sort();
    pairs
}

/// Read the attributes of `name`.
async fn describe_attributes(router: &MultiServiceRouter, name: &str) -> Wire {
    call(
        router,
        REGION,
        "DescribeLoadBalancerAttributes",
        &[param("LoadBalancerName", name)],
    )
    .await
}

/// A load balancer nobody has modified reports the documented service defaults, and
/// reports them again: a describe is a report, so it changes nothing.
#[tokio::test]
async fn a_new_load_balancer_reports_the_default_attributes() {
    let router = emulator();
    create(
        &router,
        "attribute-defaults",
        &[Listener {
            protocol: "HTTP",
            load_balancer_port: 80,
            instance_protocol: None,
            instance_port: 8080,
            ssl_certificate_id: None,
        }],
    )
    .await;

    // Every value here is from the model's own documentation: `CrossZoneLoadBalancing`
    // and `AccessLog` are `@default(false)`; "By default, Elastic Load Balancing
    // maintains a 60-second idle connection timeout"; `AccessLog$EmitInterval` "Default:
    // 60 minutes"; connection draining defaults to disabled with the 300-second timeout
    // the Classic Load Balancers Guide publishes; and the one documented additional
    // attribute `elb.http.desyncmitigationmode` has "The default is defensive".
    let expected: Vec<(String, String)> = sorted_pairs(&[
        ("AccessLog.Enabled", "false"),
        ("AccessLog.EmitInterval", "60"),
        (
            "AdditionalAttributes.elb.http.desyncmitigationmode",
            "defensive",
        ),
        ("ConnectionDraining.Enabled", "false"),
        ("ConnectionDraining.Timeout", "300"),
        ("ConnectionSettings.IdleTimeout", "60"),
        ("CrossZoneLoadBalancing.Enabled", "false"),
    ]);

    for attempt in 1..=2 {
        let described = describe_attributes(&router, "attribute-defaults").await;
        assert_eq!(
            described.status, 200,
            "attempt {attempt}: DescribeLoadBalancerAttributes must answer 200, got {} \
             with {}",
            described.status, described.body
        );
        let reported = reported_attributes(&described);
        assert_eq!(
            reported, expected,
            "attempt {attempt}: expected the documented default attributes, got \
             {reported:?}"
        );
    }

    // A name no record holds is the modelled failure, not an empty attribute set.
    let missing = describe_attributes(&router, "no-such-load-balancer").await;
    assert_eq!(
        (missing.status, missing.error_code().as_str()),
        (400, "LoadBalancerNotFound"),
        "a describe for a missing load balancer must be refused LoadBalancerNotFound, \
         got {} with {}",
        missing.status,
        missing.body
    );
}

/// `ModifyLoadBalancerAttributes` writes what the request supplies, echoes exactly those
/// members back, and leaves every other attribute as it was.
///
/// The rows run IN ORDER against one load balancer, so each one also proves that the
/// attributes the previous row wrote survived a modification that did not name them. They
/// differ along the attribute modified, the number modified at once, and the published
/// bounds of the numeric ones.
#[tokio::test]
async fn modified_attributes_are_stored_echoed_and_left_alone_by_later_calls() {
    // (case, the form fields, the `(path, value)` pairs the echo must carry, the pairs
    // the following describe must report)
    type Row = (
        &'static str,
        Vec<(String, String)>,
        Vec<(&'static str, &'static str)>,
        Vec<(&'static str, &'static str)>,
    );

    let rows: Vec<Row> = vec![
        (
            "cross-zone load balancing enabled on its own",
            vec![param(
                "LoadBalancerAttributes.CrossZoneLoadBalancing.Enabled",
                "true",
            )],
            vec![("CrossZoneLoadBalancing.Enabled", "true")],
            vec![
                ("AccessLog.Enabled", "false"),
                ("AccessLog.EmitInterval", "60"),
                (
                    "AdditionalAttributes.elb.http.desyncmitigationmode",
                    "defensive",
                ),
                ("ConnectionDraining.Enabled", "false"),
                ("ConnectionDraining.Timeout", "300"),
                ("ConnectionSettings.IdleTimeout", "60"),
                ("CrossZoneLoadBalancing.Enabled", "true"),
            ],
        ),
        (
            // The lowest value the modelled `@range(min: 1, max: 3600)` allows.
            "the idle timeout at its modelled minimum",
            vec![param(
                "LoadBalancerAttributes.ConnectionSettings.IdleTimeout",
                "1",
            )],
            vec![("ConnectionSettings.IdleTimeout", "1")],
            vec![
                ("AccessLog.Enabled", "false"),
                ("AccessLog.EmitInterval", "60"),
                (
                    "AdditionalAttributes.elb.http.desyncmitigationmode",
                    "defensive",
                ),
                ("ConnectionDraining.Enabled", "false"),
                ("ConnectionDraining.Timeout", "300"),
                ("ConnectionSettings.IdleTimeout", "1"),
                ("CrossZoneLoadBalancing.Enabled", "true"),
            ],
        ),
        (
            "the idle timeout at its modelled maximum",
            vec![param(
                "LoadBalancerAttributes.ConnectionSettings.IdleTimeout",
                "3600",
            )],
            vec![("ConnectionSettings.IdleTimeout", "3600")],
            vec![
                ("AccessLog.Enabled", "false"),
                ("AccessLog.EmitInterval", "60"),
                (
                    "AdditionalAttributes.elb.http.desyncmitigationmode",
                    "defensive",
                ),
                ("ConnectionDraining.Enabled", "false"),
                ("ConnectionDraining.Timeout", "300"),
                ("ConnectionSettings.IdleTimeout", "3600"),
                ("CrossZoneLoadBalancing.Enabled", "true"),
            ],
        ),
        (
            "connection draining enabled with its own timeout",
            vec![
                param(
                    "LoadBalancerAttributes.ConnectionDraining.Enabled",
                    "true",
                ),
                param("LoadBalancerAttributes.ConnectionDraining.Timeout", "1"),
            ],
            vec![
                ("ConnectionDraining.Enabled", "true"),
                ("ConnectionDraining.Timeout", "1"),
            ],
            vec![
                ("AccessLog.Enabled", "false"),
                ("AccessLog.EmitInterval", "60"),
                (
                    "AdditionalAttributes.elb.http.desyncmitigationmode",
                    "defensive",
                ),
                ("ConnectionDraining.Enabled", "true"),
                ("ConnectionDraining.Timeout", "1"),
                ("ConnectionSettings.IdleTimeout", "3600"),
                ("CrossZoneLoadBalancing.Enabled", "true"),
            ],
        ),
        (
            // "You can specify an interval of either 5 minutes or 60 minutes" — the
            // other published value.
            "access logs enabled with a bucket, prefix and the 5-minute interval",
            vec![
                param("LoadBalancerAttributes.AccessLog.Enabled", "true"),
                param(
                    "LoadBalancerAttributes.AccessLog.S3BucketName",
                    "my-access-logs",
                ),
                param(
                    "LoadBalancerAttributes.AccessLog.S3BucketPrefix",
                    "my-bucket-prefix/prod",
                ),
                param("LoadBalancerAttributes.AccessLog.EmitInterval", "5"),
            ],
            vec![
                ("AccessLog.Enabled", "true"),
                ("AccessLog.EmitInterval", "5"),
                ("AccessLog.S3BucketName", "my-access-logs"),
                ("AccessLog.S3BucketPrefix", "my-bucket-prefix/prod"),
            ],
            vec![
                ("AccessLog.Enabled", "true"),
                ("AccessLog.EmitInterval", "5"),
                ("AccessLog.S3BucketName", "my-access-logs"),
                ("AccessLog.S3BucketPrefix", "my-bucket-prefix/prod"),
                (
                    "AdditionalAttributes.elb.http.desyncmitigationmode",
                    "defensive",
                ),
                ("ConnectionDraining.Enabled", "true"),
                ("ConnectionDraining.Timeout", "1"),
                ("ConnectionSettings.IdleTimeout", "3600"),
                ("CrossZoneLoadBalancing.Enabled", "true"),
            ],
        ),
        (
            // The documented additional attribute, on a value the issue did not cite.
            "the desync mitigation mode set to strictest",
            vec![
                param(
                    "LoadBalancerAttributes.AdditionalAttributes.member.1.Key",
                    "elb.http.desyncmitigationmode",
                ),
                param(
                    "LoadBalancerAttributes.AdditionalAttributes.member.1.Value",
                    "strictest",
                ),
            ],
            vec![(
                "AdditionalAttributes.elb.http.desyncmitigationmode",
                "strictest",
            )],
            vec![
                ("AccessLog.Enabled", "true"),
                ("AccessLog.EmitInterval", "5"),
                ("AccessLog.S3BucketName", "my-access-logs"),
                ("AccessLog.S3BucketPrefix", "my-bucket-prefix/prod"),
                (
                    "AdditionalAttributes.elb.http.desyncmitigationmode",
                    "strictest",
                ),
                ("ConnectionDraining.Enabled", "true"),
                ("ConnectionDraining.Timeout", "1"),
                ("ConnectionSettings.IdleTimeout", "3600"),
                ("CrossZoneLoadBalancing.Enabled", "true"),
            ],
        ),
        (
            "access logs disabled again, which clears the bucket it was given",
            vec![param("LoadBalancerAttributes.AccessLog.Enabled", "false")],
            vec![("AccessLog.Enabled", "false"), ("AccessLog.EmitInterval", "60")],
            vec![
                ("AccessLog.Enabled", "false"),
                ("AccessLog.EmitInterval", "60"),
                (
                    "AdditionalAttributes.elb.http.desyncmitigationmode",
                    "strictest",
                ),
                ("ConnectionDraining.Enabled", "true"),
                ("ConnectionDraining.Timeout", "1"),
                ("ConnectionSettings.IdleTimeout", "3600"),
                ("CrossZoneLoadBalancing.Enabled", "true"),
            ],
        ),
    ];

    let router = emulator();
    create(
        &router,
        "attribute-round-trip",
        &[Listener {
            protocol: "HTTP",
            load_balancer_port: 80,
            instance_protocol: None,
            instance_port: 8080,
            ssl_certificate_id: None,
        }],
    )
    .await;

    for (case, fields, echoed, stored) in rows {
        let mut params = vec![param("LoadBalancerName", "attribute-round-trip")];
        params.extend(fields);
        let modified = call(&router, REGION, "ModifyLoadBalancerAttributes", &params).await;
        assert_eq!(
            modified.status, 200,
            "case {case:?}: ModifyLoadBalancerAttributes must answer 200, got {} with {}",
            modified.status, modified.body
        );
        let doc = parse(&modified.body);
        let result = modified.result(&doc);
        assert_eq!(
            kid_text(result, "LoadBalancerName"),
            Some("attribute-round-trip".to_string()),
            "case {case:?}: the answer must echo LoadBalancerName, got {}",
            modified.body
        );
        let want_echo: Vec<(String, String)> = sorted_pairs(&echoed);
        let got_echo = reported_attributes(&modified);
        assert_eq!(
            got_echo, want_echo,
            "case {case:?}: the answer must echo exactly the attributes the request \
             supplied, got {got_echo:?}"
        );

        let described = describe_attributes(&router, "attribute-round-trip").await;
        let want_stored: Vec<(String, String)> = sorted_pairs(&stored);
        let got_stored = reported_attributes(&described);
        assert_eq!(
            got_stored, want_stored,
            "case {case:?}: the describe must report the modification and leave every \
             other attribute as it was, got {got_stored:?}"
        );
    }
}

/// `(path, value)` pairs in the order [`reported_attributes`] returns them.
fn sorted_pairs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut owned: Vec<(String, String)> = pairs
        .iter()
        .map(|(path, value)| ((*path).to_string(), (*value).to_string()))
        .collect();
    owned.sort();
    owned
}

/// An attribute value the service cannot apply is refused, and the stored attributes are
/// left alone. Each numeric row is one past a published bound.
#[tokio::test]
async fn attribute_values_the_rules_rule_out_are_refused() {
    let router = emulator();
    create(
        &router,
        "attribute-refusals",
        &[Listener {
            protocol: "HTTP",
            load_balancer_port: 80,
            instance_protocol: None,
            instance_port: 8080,
            ssl_certificate_id: None,
        }],
    )
    .await;
    let before = reported_attributes(&describe_attributes(&router, "attribute-refusals").await);

    // (case, form fields, expected status, expected code — `None` where the refusal is
    // the framework's own, for a constraint the model declares)
    let rows: [RefusalRow; 8] = [
        (
            "an idle timeout one below the modelled minimum",
            vec![param(
                "LoadBalancerAttributes.ConnectionSettings.IdleTimeout",
                "0",
            )],
            400,
            None,
        ),
        (
            "an idle timeout one above the modelled maximum",
            vec![param(
                "LoadBalancerAttributes.ConnectionSettings.IdleTimeout",
                "3601",
            )],
            400,
            None,
        ),
        (
            "a connection-draining timeout one below the published minimum",
            vec![
                param("LoadBalancerAttributes.ConnectionDraining.Enabled", "true"),
                param("LoadBalancerAttributes.ConnectionDraining.Timeout", "0"),
            ],
            409,
            Some("InvalidConfigurationRequest"),
        ),
        (
            "a connection-draining timeout one above the published maximum",
            vec![
                param("LoadBalancerAttributes.ConnectionDraining.Enabled", "true"),
                param("LoadBalancerAttributes.ConnectionDraining.Timeout", "3601"),
            ],
            409,
            Some("InvalidConfigurationRequest"),
        ),
        (
            "an access-log interval that is neither published value",
            vec![
                param("LoadBalancerAttributes.AccessLog.Enabled", "true"),
                param("LoadBalancerAttributes.AccessLog.S3BucketName", "logs"),
                param("LoadBalancerAttributes.AccessLog.EmitInterval", "30"),
            ],
            409,
            Some("InvalidConfigurationRequest"),
        ),
        (
            "access logs enabled with no bucket to deliver them to",
            vec![param("LoadBalancerAttributes.AccessLog.Enabled", "true")],
            409,
            Some("InvalidConfigurationRequest"),
        ),
        (
            "an additional attribute this service does not publish",
            vec![
                param(
                    "LoadBalancerAttributes.AdditionalAttributes.member.1.Key",
                    "elb.http.nosuchattribute",
                ),
                param(
                    "LoadBalancerAttributes.AdditionalAttributes.member.1.Value",
                    "monitor",
                ),
            ],
            400,
            Some("LoadBalancerAttributeNotFound"),
        ),
        (
            "a desync mitigation mode outside the documented values",
            vec![
                param(
                    "LoadBalancerAttributes.AdditionalAttributes.member.1.Key",
                    "elb.http.desyncmitigationmode",
                ),
                param(
                    "LoadBalancerAttributes.AdditionalAttributes.member.1.Value",
                    "reckless",
                ),
            ],
            409,
            Some("InvalidConfigurationRequest"),
        ),
    ];

    for (case, fields, status, code) in rows {
        let mut params = vec![param("LoadBalancerName", "attribute-refusals")];
        params.extend(fields);
        let refused = call(&router, REGION, "ModifyLoadBalancerAttributes", &params).await;
        assert_eq!(
            refused.status, status,
            "case {case:?}: expected HTTP {status}, got {} with {}",
            refused.status, refused.body
        );
        assert!(
            refused.body.contains("<ErrorResponse"),
            "case {case:?}: an awsQuery client must get an <ErrorResponse> envelope, got {}",
            refused.body
        );
        if let Some(code) = code {
            assert_eq!(
                refused.error_code(),
                code,
                "case {case:?}: expected the error code {code}, got {}",
                refused.body
            );
        }
        let after = reported_attributes(&describe_attributes(&router, "attribute-refusals").await);
        assert_eq!(
            after, before,
            "case {case:?}: a refused modification must leave every attribute as it was, \
             got {after:?}"
        );
    }

    // A modification for a load balancer no record holds is the modelled failure.
    let missing = call(
        &router,
        REGION,
        "ModifyLoadBalancerAttributes",
        &[
            param("LoadBalancerName", "no-such-load-balancer"),
            param(
                "LoadBalancerAttributes.CrossZoneLoadBalancing.Enabled",
                "true",
            ),
        ],
    )
    .await;
    assert_eq!(
        (missing.status, missing.error_code().as_str()),
        (400, "LoadBalancerNotFound"),
        "a modification for a missing load balancer must be refused LoadBalancerNotFound, \
         got {} with {}",
        missing.status,
        missing.body
    );
}
