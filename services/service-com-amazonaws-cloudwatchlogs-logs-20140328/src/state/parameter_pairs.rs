// SPDX-License-Identifier: CC0-1.0

//! How this service words a refusal about a PAIR of input parameters, in one place.
//!
//! A constraint the model states on one member is refused through the validation envelope
//! (`crate::service`): a clause naming the member and the constraint it failed. A
//! requirement that relates TWO members — these two cannot be combined, these two have to
//! arrive together, one of these two is required — is not a member constraint at all, so no
//! clause can state it; CloudWatch Logs answers each of those with a FIXED sentence of its
//! own, naming the two parameters and nothing from the request.
//!
//! Three sentence forms cover every such pair this service checks, and each is stated here
//! once so a handler states AWS's sentence rather than authoring a paraphrase of it:
//!
//! * [`mutually_exclusive`] — the two cannot be combined, as the recorded refusals for the
//!   log-group name/ARN selector pair and for the live-tail stream-name/prefix pair are
//!   worded;
//! * [`must_contain_both`] — the two are meaningful only together, as the recorded
//!   `DescribeMetricFilters` refusal for a half-named metric is worded, naming the request
//!   by the operation's words;
//! * [`either_required`] — one of the two has to be named, as the recorded refusals for the
//!   log-group selector pair and for the lookup-table source pair are worded;
//! * [`cannot_be_combined`] — the two cannot be combined, as the recorded `StartQuery`
//!   refusal for two of its three log-group members is worded, which states the pair as a
//!   pair rather than as a rule over a list.
//!
//! Each form takes the parameters as the service NAMES them, which is a property of the
//! pair: the selector pair is stated in the service's display words (`LogGroup name`), the
//! metric pair in the capitalised member names AWS states them in, and the live-tail pair in
//! the member spellings — so the names are the caller's choice and the sentence is not.
//! Nothing of the submitted value is interpolated into any of them.

/// The sentence for two parameters that cannot be combined: `<first> and <second> are
/// mutually exclusive parameters.`, trailing period included.
pub fn mutually_exclusive(first: &str, second: &str) -> String {
    format!("{first} and {second} are mutually exclusive parameters.")
}

/// The sentence for two parameters that are meaningful only together: `<request> request
/// must contain both <first> and <second>`, with NO trailing period — `request` names the
/// request in the operation's own words, as AWS spaces them.
pub fn must_contain_both(request: &str, first: &str, second: &str) -> String {
    format!("{request} request must contain both {first} and {second}")
}

/// The sentence for a pair one of which has to be named: `Either <first> or <second> must be
/// provided.`, trailing period included.
pub fn either_required(first: &str, second: &str) -> String {
    format!("Either {first} or {second} must be provided.")
}

/// The second sentence for two parameters that cannot be combined: `Both <first> and <second>
/// cannot be specified at the same time`, with NO trailing period — the wording the recorded
/// `StartQuery` refusal for two log-group members carries, which names the pair the request
/// combined rather than the whole list the operation chooses from.
pub fn cannot_be_combined(first: &str, second: &str) -> String {
    format!("Both {first} and {second} cannot be specified at the same time")
}

#[cfg(test)]
mod tests {
    use super::{cannot_be_combined, either_required, must_contain_both, mutually_exclusive};

    /// Each form states the two parameters it is given, in the order it is given them, and
    /// nothing else: the fixed words, the punctuation and the spacing are the form's own.
    #[test]
    fn each_form_states_its_two_parameters_and_nothing_else() {
        let exclusive = [
            // The recorded log-group selector refusal, stated in the service's display words.
            (
                "LogGroup name",
                "LogGroup ARN",
                "LogGroup name and LogGroup ARN are mutually exclusive parameters.",
            ),
            // The recorded `DescribeLogGroups` refusal for the two name filters.
            (
                "LogGroup name prefix",
                "LogGroup name pattern",
                "LogGroup name prefix and LogGroup name pattern are mutually exclusive \
                 parameters.",
            ),
            // The recorded `StartLiveTail` refusal, stated in the member spellings.
            (
                "logStreamNames",
                "logStreamNamePrefixes",
                "logStreamNames and logStreamNamePrefixes are mutually exclusive parameters.",
            ),
        ];
        for (first, second, expected) in exclusive {
            let actual = mutually_exclusive(first, second);
            assert_eq!(
                actual, expected,
                "case {first}/{second}: expected {expected:?}, got {actual:?}"
            );
        }

        let paired = [
            // The recorded `DescribeMetricFilters` refusal for a half-named metric.
            (
                "Describe Metric Filters",
                "MetricName",
                "MetricNamespace",
                "Describe Metric Filters request must contain both MetricName and \
                 MetricNamespace",
            ),
            (
                "Describe Log Streams",
                "LogStreamName",
                "LogStreamNamePrefix",
                "Describe Log Streams request must contain both LogStreamName and \
                 LogStreamNamePrefix",
            ),
        ];
        for (request, first, second, expected) in paired {
            let actual = must_contain_both(request, first, second);
            assert_eq!(
                actual, expected,
                "case {request} {first}/{second}: expected {expected:?}, got {actual:?}"
            );
        }

        let required = [
            // The recorded log-group selector refusal for a request naming neither.
            (
                "LogGroupName",
                "LogGroupArn",
                "Either LogGroupName or LogGroupArn must be provided.",
            ),
            // The recorded lookup-table source refusal.
            (
                "tableBody",
                "queryId",
                "Either tableBody or queryId must be provided.",
            ),
        ];
        for (first, second, expected) in required {
            let actual = either_required(first, second);
            assert_eq!(
                actual, expected,
                "case {first}/{second}: expected {expected:?}, got {actual:?}"
            );
        }

        let combined = [
            // The recorded `StartQuery` refusal for two of its three log-group members.
            (
                "logGroupName",
                "logGroupNames",
                "Both logGroupName and logGroupNames cannot be specified at the same time",
            ),
            (
                "logGroupNames",
                "logGroupIdentifiers",
                "Both logGroupNames and logGroupIdentifiers cannot be specified at the same time",
            ),
        ];
        for (first, second, expected) in combined {
            let actual = cannot_be_combined(first, second);
            assert_eq!(
                actual, expected,
                "case {first}/{second}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
