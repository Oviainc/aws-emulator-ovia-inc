// SPDX-License-Identifier: CC0-1.0

//! Which endpoint the `GetSuiteRunReport` download URL names, over every way a caller can
//! address this emulator.
//!
//! The rule under test: the endpoint a URL this service announces is resolved from the
//! request by testing each source in turn — the `Host` header first, then the authority
//! the request URI carries — and announcing the FIRST source that carries a bare
//! authority (a host with an optional port and nothing else), falling back to the loopback
//! literal only when NEITHER source carries one. A `Host` value this service cannot
//! honour therefore leaves the authority the request URI carries still to be read, rather
//! than consuming the chain, which is what makes an announced URL resolve back to the
//! endpoint the caller actually reached.
//!
//! The table spans what the rule generalises over: which source carries the endpoint
//! (the header, the URI, both, neither), the forms a bare authority takes (a service
//! endpoint host, a host with an explicit port, a loopback socket, a loopback name, an
//! IPv6 literal), and the forms that are not an endpoint at all (a path, a query,
//! userinfo, a backslash, a space, a tab, the empty value, and bytes that are not UTF-8),
//! each of those crossed with a request URI that does and does not carry an authority of
//! its own — including an unusable authority on the URI, so the filter is pinned on the
//! second source too.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter` the
//! emulator serves on a socket.

use service_com_amazonaws_iotdeviceadvisor_iotsenateservice as iotdeviceadvisor;

use super::suite_lifecycle::{Answer, Emulator};
use super::suite_run_report::{definition, seed_run, REGION};

/// The endpoint an announced URL names when the request carries none this service can
/// honour. Mirrors the loopback literal `crate::state` announces in that case, which is
/// not public, so a drift between the two shows up as a failure here.
const LOOPBACK_HOST: &str = "127.0.0.1";

/// A service endpoint host for the region every case signs for.
const SERVICE_HOST: &str = "api.iotdeviceadvisor.us-east-1.amazonaws.com";

/// How one case addresses the emulator, and the endpoint the URL it is answered with has
/// to name.
struct Addressing {
    label: &'static str,
    /// The RAW bytes of the `Host` header the request carries, or `None` for a request
    /// that sends no `Host` header at all (as an HTTP/2 request does).
    host: Option<&'static [u8]>,
    /// The authority the request URI carries in absolute form, or `None` for an
    /// origin-form request URI, which carries none.
    authority: Option<&'static str>,
    /// The authority the announced URL must name.
    announced: &'static str,
}

/// The ways a caller reaches this emulator, each with the endpoint its report URL has to
/// name.
fn addressings() -> Vec<Addressing> {
    vec![
        Addressing {
            label: "a service endpoint host in the Host header",
            host: Some(b"api.iotdeviceadvisor.us-east-1.amazonaws.com"),
            authority: None,
            announced: SERVICE_HOST,
        },
        Addressing {
            label: "a Host header with an explicit port",
            host: Some(b"api.iotdeviceadvisor.us-east-1.amazonaws.com:8443"),
            authority: None,
            announced: "api.iotdeviceadvisor.us-east-1.amazonaws.com:8443",
        },
        Addressing {
            label: "the emulator's own socket",
            host: Some(b"127.0.0.1:9297"),
            authority: None,
            announced: "127.0.0.1:9297",
        },
        Addressing {
            label: "a loopback name with a port",
            host: Some(b"localhost:9297"),
            authority: None,
            announced: "localhost:9297",
        },
        Addressing {
            label: "an IPv6 literal with a port",
            host: Some(b"[::1]:9297"),
            authority: None,
            announced: "[::1]:9297",
        },
        Addressing {
            label: "an endpoint carried only on the request URI",
            host: None,
            authority: Some("api.iotdeviceadvisor.us-east-1.amazonaws.com:9297"),
            announced: "api.iotdeviceadvisor.us-east-1.amazonaws.com:9297",
        },
        Addressing {
            label: "both sources usable: the Host header is the one the caller addressed",
            host: Some(b"localhost:9297"),
            authority: Some("127.0.0.1:443"),
            announced: "localhost:9297",
        },
        Addressing {
            label: "a Host carrying a path, the endpoint on the URI",
            host: Some(b"api.iotdeviceadvisor.us-east-1.amazonaws.com/qualificationReports"),
            authority: Some("127.0.0.1:9297"),
            announced: "127.0.0.1:9297",
        },
        Addressing {
            label: "a Host carrying a query, the endpoint on the URI",
            host: Some(b"elsewhere.example.com?x=1"),
            authority: Some("127.0.0.1:9297"),
            announced: "127.0.0.1:9297",
        },
        Addressing {
            label: "a Host carrying a fragment, the endpoint on the URI",
            host: Some(b"elsewhere.example.com#report"),
            authority: Some("localhost:9297"),
            announced: "localhost:9297",
        },
        Addressing {
            label: "a Host carrying userinfo, the endpoint on the URI",
            host: Some(b"user@elsewhere.example.com"),
            authority: Some("127.0.0.1:9297"),
            announced: "127.0.0.1:9297",
        },
        Addressing {
            label: "a Host carrying a backslash, the endpoint on the URI",
            host: Some(br"elsewhere.example.com\report"),
            authority: Some("127.0.0.1:9297"),
            announced: "127.0.0.1:9297",
        },
        Addressing {
            label: "a Host carrying a space, the endpoint on the URI",
            host: Some(b"elsewhere example com"),
            authority: Some("127.0.0.1:9297"),
            announced: "127.0.0.1:9297",
        },
        Addressing {
            label: "a Host carrying a tab, the endpoint on the URI",
            host: Some(b"elsewhere\texample.com"),
            authority: Some("127.0.0.1:9297"),
            announced: "127.0.0.1:9297",
        },
        Addressing {
            label: "an empty Host, the endpoint on the URI",
            host: Some(b""),
            authority: Some("127.0.0.1:9297"),
            announced: "127.0.0.1:9297",
        },
        Addressing {
            label: "a Host that is not UTF-8, the endpoint on the URI",
            host: Some(&[0xff, 0xfe]),
            authority: Some("127.0.0.1:9297"),
            announced: "127.0.0.1:9297",
        },
        Addressing {
            label: "a Host carrying a path and no authority on the URI",
            host: Some(b"elsewhere.example.com/report"),
            authority: None,
            announced: LOOPBACK_HOST,
        },
        Addressing {
            label: "an empty Host and no authority on the URI",
            host: Some(b""),
            authority: None,
            announced: LOOPBACK_HOST,
        },
        Addressing {
            label: "a Host that is not UTF-8 and no authority on the URI",
            host: Some(&[0xff, 0xfe]),
            authority: None,
            announced: LOOPBACK_HOST,
        },
        Addressing {
            label: "neither source carries anything",
            host: None,
            authority: None,
            announced: LOOPBACK_HOST,
        },
        Addressing {
            label: "no Host and a URI authority carrying userinfo",
            host: None,
            authority: Some("user@127.0.0.1:9297"),
            announced: LOOPBACK_HOST,
        },
        Addressing {
            label: "a Host carrying a path and a URI authority carrying userinfo",
            host: Some(b"elsewhere.example.com/report"),
            authority: Some("user@127.0.0.1:9297"),
            announced: LOOPBACK_HOST,
        },
    ]
}

/// `GetSuiteRunReport` for one `(definition, run)` pair, addressed the way `addressing`
/// asks.
async fn report(emulator: &Emulator, addressing: &Addressing, path: &str) -> Answer {
    emulator
        .call_addressed(
            "GET",
            path,
            REGION,
            addressing.host,
            addressing.authority,
        )
        .await
}

#[tokio::test]
async fn an_announced_report_url_names_the_endpoint_the_request_reached() {
    let emulator = Emulator::new();
    let definition_id = definition(&emulator, "report-url-endpoint-suite", Some(true)).await;
    let run_id = seed_run(&emulator, &definition_id, "PASS");
    let path = format!("/suiteDefinitions/{definition_id}/suiteRuns/{run_id}/report");

    for addressing in addressings() {
        let label = addressing.label;
        let answer = report(&emulator, &addressing, &path).await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: a successful run of a qualification suite has a report, got {} {}",
            answer.status, answer.body
        );

        let expected = format!(
            "https://{}/qualificationReports/{definition_id}/{run_id}",
            addressing.announced
        );
        assert_eq!(
            answer.member("qualificationReportDownloadUrl").as_str(),
            Some(expected.as_str()),
            "case {label:?}: the announced URL must name the endpoint the request reached \
             ({expected:?}), got {}",
            answer.body
        );
    }
}

/// The endpoint is resolved per REQUEST, not remembered: the same run read through two
/// endpoints announces each of them, and the record itself is unchanged by either read.
#[tokio::test]
async fn the_announced_endpoint_follows_the_request_and_is_not_stored() {
    let emulator = Emulator::new();
    let definition_id = definition(&emulator, "per-request-endpoint-suite", Some(true)).await;
    let run_id = seed_run(&emulator, &definition_id, "PASS_WITH_WARNINGS");
    let path = format!("/suiteDefinitions/{definition_id}/suiteRuns/{run_id}/report");
    let run_path = format!("/suiteDefinitions/{definition_id}/suiteRuns/{run_id}");

    let before = emulator.call("GET", &run_path, REGION, None).await;
    let announced: Vec<String> = {
        let mut announced = Vec::new();
        for addressing in [
            Addressing {
                label: "the service endpoint",
                host: Some(b"api.iotdeviceadvisor.us-east-1.amazonaws.com"),
                authority: None,
                announced: SERVICE_HOST,
            },
            Addressing {
                label: "an unusable Host over a socket on the URI",
                host: Some(b"api.iotdeviceadvisor.us-east-1.amazonaws.com/x"),
                authority: Some("127.0.0.1:9297"),
                announced: "127.0.0.1:9297",
            },
        ] {
            let answer = report(&emulator, &addressing, &path).await;
            assert_eq!(
                answer.member("qualificationReportDownloadUrl").as_str(),
                Some(
                    format!(
                        "https://{}/qualificationReports/{definition_id}/{run_id}",
                        addressing.announced
                    )
                    .as_str()
                ),
                "case {:?}: the announced URL must name the endpoint of THIS request, got {}",
                addressing.label,
                answer.body
            );
            announced.push(answer.string("qualificationReportDownloadUrl"));
        }
        announced
    };
    assert_ne!(
        announced[0], announced[1],
        "two different endpoints must be announced differently, got {announced:?}"
    );

    let after = emulator.call("GET", &run_path, REGION, None).await;
    assert_eq!(
        after.body, before.body,
        "reading the report must not change what a later read of the run answers"
    );
    // And the store holds no announced URL of its own: the run reads back the same
    // whichever endpoint the report was read through.
    let state = iotdeviceadvisor::state::service_state(&*emulator.engine)
        .expect("the service's store is reachable from the shared engine");
    let held = state.lock().runs.len();
    assert_eq!(
        held, 1,
        "reading a report through two endpoints must not add a record, got {held} runs"
    );
}
