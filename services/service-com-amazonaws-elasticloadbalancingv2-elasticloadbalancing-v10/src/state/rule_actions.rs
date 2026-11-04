// SPDX-License-Identifier: CC0-1.0

//! What a REDIRECT action's target URI is read as, and what makes one an action this
//! service refuses.
//!
//! A redirect action names the components of the URI the client is sent to — "A URI
//! consists of the following components: protocol://hostname:port/path?query" — and the
//! components it leaves out "retain their original values", which the reserved keywords
//! `#{protocol}`, `#{host}`, `#{port}`, `#{path}` and `#{query}` are the spelling of. So
//! one reading of those keywords serves both halves of the operation: what a read REPORTS
//! for a component the request did not modify, and whether the action modifies anything
//! at all.
//!
//! Judged wherever a redirect action is SET — a rule's actions and a listener's default
//! actions, on the create that first stores them and on the modification that replaces
//! them — so one reading of the clause serves every operation that takes one.

use crate::ops::types::Action;

/// The reserved keyword each component of a redirect action's target URI is REPORTED as
/// when the request did not modify that component, from `RedirectActionConfig`'s own
/// documentation: "A URI consists of the following components:
/// protocol://hostname:port/path?query. … Any components that you do not modify retain
/// their original values. You can reuse URI components using the following reserved
/// keywords: #{protocol}, #{host}, #{port}, #{path} (the leading "/" is removed),
/// #{query}."
///
/// So a component left out keeps the request's own value, and the keyword for it is what
/// says so. `Path` is the one with a form of its own: the keyword drops the leading "/",
/// which the member itself must carry ("The absolute path, starting with the leading
/// "/""), so the path that retains its original value is `/#{path}`.
pub const KEEP_PROTOCOL: &str = "#{protocol}";
pub const KEEP_HOST: &str = "#{host}";
pub const KEEP_PORT: &str = "#{port}";
pub const KEEP_PATH: &str = "/#{path}";
pub const KEEP_QUERY: &str = "#{query}";

/// The keyword written WITHOUT the leading "/" the `Path` member carries: the reserved
/// keyword itself, which a request may write in place of a path and which retains the
/// request's own path just as [`KEEP_PATH`] does.
///
/// Public because the data plane SUBSTITUTES that bare keyword when it renders a redirect
/// (`crate::service::data_plane`), so the plane that resolves a component and the
/// validation that judges one read the same spelling.
pub const KEEP_PATH_KEYWORD: &str = "#{path}";

/// The refusal a redirect that modifies none of the components the client is sent to
/// earns, which the model's own clause states: "You must modify at least one of the
/// following components to avoid a redirect loop: protocol, hostname, port, or path."
///
/// Fixed text: the rule is about the configuration as a whole, so there is no one value of
/// the request to name in it.
pub const REDIRECT_LOOP_MESSAGE: &str =
    "The redirect configuration is not valid because it creates a loop.";

/// The placeholders `RedirectActionConfig$Path` may reuse: "The path can contain #{host},
/// #{path}, and #{port}" — the three of the five reserved keywords that component accepts,
/// each at most once. Named through the keywords above, so the module holds ONE reading of
/// each reserved keyword.
const PATH_PLACEHOLDERS: &[&str] = &[KEEP_PATH_KEYWORD, KEEP_HOST, KEEP_PORT];

/// The refusal a `Path` that is not an absolute path, or that reuses a URI component the
/// path may not reuse, earns.
///
/// Fixed text naming the member and the placeholders it accepts: the rule is about the
/// FORM of that one component ("The absolute path, starting with the leading "/" … The
/// path can contain #{host}, #{path}, and #{port}"), so the refusal states the form rather
/// than the value that broke it.
pub const REDIRECT_PATH_MESSAGE: &str = "The Path parameter must be a valid path, should \
     start with a '/', and may contain up to one of each of these placeholders: \
     '#{path}', '#{host}', '#{port}'.";

/// The ports a redirect may send the client to, from `RedirectActionConfig$Port`: "You
/// can specify a value from 1 to 65535 or #{port}."
const LOWEST_REDIRECT_PORT: u32 = 1;
const HIGHEST_REDIRECT_PORT: u32 = 65_535;

/// The refusal a `Port` that is neither a port number nor the keyword that retains the
/// request's own port earns. Fixed text naming the member, the bounds and the keyword: the
/// rule is the form of that one component, so the refusal states the form rather than the
/// value that broke it.
pub const REDIRECT_PORT_MESSAGE: &str = "The Port parameter must be either an integer \
     between 1 and 65535 or '#{port}'.";

/// Enforce the FORM each component of a redirect's target URI has, which the model states
/// in prose and declares no error shape for beyond the invalid-action one every operation
/// that takes an action models.
///
/// The path is absolute ("The absolute path, starting with the leading "/"") and reuses
/// only the components a path may reuse, once each; the port is a port number inside the
/// published range or the keyword that retains the request's own ("You can specify a value
/// from 1 to 65535 or #{port}"). A component the request leaves out retains the request's
/// own value and so has no form to break.
pub fn check_redirect_actions(actions: &[Action]) -> Result<(), String> {
    for config in actions
        .iter()
        .filter_map(|action| action.redirect_config.as_ref())
    {
        if let Some(path) = config.path.as_deref() {
            if !path.starts_with('/') || !placeholders_allowed_once(path, PATH_PLACEHOLDERS) {
                return Err(REDIRECT_PATH_MESSAGE.to_string());
            }
        }
        if let Some(port) = config.port.as_deref() {
            if !is_redirect_port(port) {
                return Err(REDIRECT_PORT_MESSAGE.to_string());
            }
        }
    }
    Ok(())
}

/// Whether one value is a port a redirect may send the client to: a port number inside the
/// published range, or the keyword that retains the request's own port — and nothing else,
/// a number with a sign or surrounding text included.
fn is_redirect_port(port: &str) -> bool {
    if port == KEEP_PORT {
        return true;
    }
    port.parse::<u32>()
        .is_ok_and(|port| (LOWEST_REDIRECT_PORT..=HIGHEST_REDIRECT_PORT).contains(&port))
}

/// Whether every reserved keyword written into one component is one that component may
/// reuse and is written at most once — and that nothing in it opens a keyword it never
/// closes.
///
/// Every step reads the component through a CHECKED slice, so any bytes a request carries
/// are judged rather than able to break the read.
fn placeholders_allowed_once(component: &str, allowed: &[&str]) -> bool {
    let mut seen: Vec<&str> = Vec::new();
    let mut rest = component;
    while let Some(open) = rest.find("#{") {
        let Some(from_open) = rest.get(open..) else {
            return false;
        };
        let Some(close) = from_open.find('}') else {
            return false;
        };
        let (Some(keyword), Some(after)) = (from_open.get(..=close), from_open.get(close + 1..))
        else {
            return false;
        };
        if !allowed.contains(&keyword) || seen.contains(&keyword) {
            return false;
        }
        seen.push(keyword);
        rest = after;
    }
    true
}

/// Whether an action list carries a redirect that sends the client back to where it
/// already is: one whose protocol, hostname, port and path are each either left out or
/// written as the keyword that retains the request's own value.
///
/// The query is NOT one of them — the model names the four components whose modification
/// avoids a loop, and the query string is not among them — so a redirect that changes only
/// the query loops as surely as one that changes nothing.
pub fn redirects_to_itself(actions: &[Action]) -> bool {
    actions
        .iter()
        .filter_map(|action| action.redirect_config.as_ref())
        .any(|config| {
            unmodified(config.protocol.as_deref(), &[KEEP_PROTOCOL])
                && unmodified(config.host.as_deref(), &[KEEP_HOST])
                && unmodified(config.port.as_deref(), &[KEEP_PORT])
                && unmodified(config.path.as_deref(), &[KEEP_PATH, KEEP_PATH_KEYWORD])
        })
}

/// Whether one component of a redirect's target URI is left as the request's own: either
/// not named at all, or named as a keyword that retains it.
fn unmodified(component: Option<&str>, keywords: &[&str]) -> bool {
    component.is_none_or(|value| keywords.contains(&value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::types::RedirectActionConfig;

    /// One redirect, written as the five components a request may name.
    fn redirect(
        protocol: Option<&str>,
        host: Option<&str>,
        port: Option<&str>,
        path: Option<&str>,
        query: Option<&str>,
    ) -> Vec<Action> {
        vec![Action {
            r#type: "redirect".to_string(),
            redirect_config: Some(RedirectActionConfig {
                protocol: protocol.map(str::to_string),
                host: host.map(str::to_string),
                port: port.map(str::to_string),
                path: path.map(str::to_string),
                query: query.map(str::to_string),
                status_code: "HTTP_301".to_string(),
            }),
            ..Action::default()
        }]
    }

    /// A redirect loops exactly when none of the four components the model names is
    /// modified — whatever the query says, and however the retained components are
    /// spelled.
    #[test]
    fn a_redirect_loops_only_when_none_of_the_four_components_is_modified() {
        // Each row: the case, the five components, and whether that redirect loops.
        let rows: &[(&str, Vec<Action>, bool)] = &[
            ("nothing named at all", redirect(None, None, None, None, None), true),
            (
                "every component written as its keyword",
                redirect(
                    Some(KEEP_PROTOCOL),
                    Some(KEEP_HOST),
                    Some(KEEP_PORT),
                    Some(KEEP_PATH),
                    Some(KEEP_QUERY),
                ),
                true,
            ),
            (
                "the path keyword without its leading slash",
                redirect(None, None, None, Some(KEEP_PATH_KEYWORD), None),
                true,
            ),
            (
                "only the query modified",
                redirect(None, None, None, None, Some("#{query}&value=xyz")),
                true,
            ),
            (
                "the protocol modified",
                redirect(Some("HTTPS"), None, None, None, None),
                false,
            ),
            (
                "the host modified",
                redirect(None, Some("moved.example.com"), None, None, None),
                false,
            ),
            (
                "the port modified",
                redirect(None, None, Some("8443"), None, None),
                false,
            ),
            (
                "the path modified while reusing the original",
                redirect(None, None, None, Some("/new/#{path}"), None),
                false,
            ),
            (
                "every component modified",
                redirect(
                    Some("HTTPS"),
                    Some("all.example.com"),
                    Some("443"),
                    Some("/landing"),
                    Some("from=elb"),
                ),
                false,
            ),
        ];
        for (case, actions, loops) in rows {
            assert_eq!(
                redirects_to_itself(actions),
                *loops,
                "case {case:?}: expected loops={loops}, got {:?}",
                !*loops
            );
        }

        // An action list with no redirect in it at all loops never.
        assert!(
            !redirects_to_itself(&[Action {
                r#type: "fixed-response".to_string(),
                ..Action::default()
            }]),
            "case a fixed response: expected no loop"
        );
    }

    /// A redirect's path is absolute and reuses only the components a path may reuse,
    /// once each.
    #[test]
    fn a_redirect_path_is_absolute_and_reuses_each_component_at_most_once() {
        // Each row: the case, the Path the request carries, and whether it is accepted.
        let rows: &[(&str, Option<&str>, bool)] = &[
            ("no path at all, which retains the original", None, true),
            ("an absolute path", Some("/landing"), true),
            ("the keyword that retains the original", Some(KEEP_PATH), true),
            ("a path that reuses the original", Some("/new/#{path}"), true),
            // Each placeholder the member's own documentation names, and all three at once.
            ("the host placeholder", Some("/#{host}/landing"), true),
            ("the port placeholder", Some("/p/#{port}"), true),
            (
                "all three placeholders a path may reuse",
                Some("/#{host}/#{port}/#{path}"),
                true,
            ),
            ("no leading slash", Some("no-leading-slash"), false),
            ("a relative path with a placeholder", Some("#{path}"), false),
            (
                "a placeholder written twice",
                Some("/#{path}/#{path}"),
                false,
            ),
            (
                "a placeholder the path may not reuse",
                Some("/#{query}"),
                false,
            ),
            (
                "the protocol placeholder, which a path may not reuse",
                Some("/#{protocol}"),
                false,
            ),
            ("a keyword that is never closed", Some("/#{path"), false),
        ];
        for (case, path, accepted) in rows {
            let actions = redirect(Some("HTTPS"), None, None, *path, None);
            let answer = check_redirect_actions(&actions);
            assert_eq!(
                answer.is_ok(),
                *accepted,
                "case {case:?}: expected accepted={accepted}, got {answer:?}"
            );
            if !*accepted {
                assert_eq!(
                    answer.as_ref().err().map(String::as_str),
                    Some(REDIRECT_PATH_MESSAGE),
                    "case {case:?}: expected the refusal the member's form earns, got {answer:?}"
                );
            }
        }
    }

    /// A redirect's port is a port number inside the published range, or the keyword that
    /// retains the request's own port.
    #[test]
    fn a_redirect_port_is_a_port_number_or_the_keyword_that_retains_it() {
        // Each row: the case, the Port the request carries, and whether it is accepted.
        // The rows cover both published bounds, one past each, and the forms a value that
        // is not a port takes.
        let rows: &[(&str, Option<&str>, bool)] = &[
            ("no port at all, which retains the original", None, true),
            ("the keyword that retains the original", Some(KEEP_PORT), true),
            ("the lowest port", Some("1"), true),
            ("the highest port", Some("65535"), true),
            ("a port in the middle", Some("8443"), true),
            ("one below the lowest port", Some("0"), false),
            ("one past the highest port", Some("65536"), false),
            ("a negative port", Some("-1"), false),
            ("a port that is not a number", Some("http"), false),
            ("a port with surrounding text", Some("port 8443"), false),
            ("a fractional port", Some("8443.0"), false),
            ("another component's keyword", Some("#{path}"), false),
        ];
        for (case, port, accepted) in rows {
            let actions = redirect(None, None, *port, Some("/landing"), None);
            let answer = check_redirect_actions(&actions);
            assert_eq!(
                answer.is_ok(),
                *accepted,
                "case {case:?}: expected accepted={accepted}, got {answer:?}"
            );
            if !*accepted {
                assert_eq!(
                    answer.as_ref().err().map(String::as_str),
                    Some(REDIRECT_PORT_MESSAGE),
                    "case {case:?}: expected the refusal the member's form earns, got {answer:?}"
                );
            }
        }
    }
}
