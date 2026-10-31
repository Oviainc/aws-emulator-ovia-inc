Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: dbe587d9dbd8151bbbae38161490f3fbca8aac16

# The ValidationException clause reports a vacuous regex for LogGroupArn: unanchored() strips the anchors of an alternation's branches

_Filed: 2026-10-28T17:43:47Z_
_Operation: UpdateHypervisor | Class: error-response_

> _Note: this issue was identified against the code base at commit `dbe587d9dbd8151bbbae38161490f3fbca8aac16`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::service::constraint_clause` renders a `@pattern` violation by passing the modelled expression through `unanchored()`, whose stated rule is that it removes "the `^` / `$` the model spells it with" so the clause "states the character class a value must satisfy, not the anchoring the model uses to make the match total". The implementation removes exactly ONE leading `^` and ONE trailing `$` from the whole string, which is only correct when the pattern is a single anchored branch.

This model has a member whose pattern is an alternation of two separately anchored branches: `UpdateHypervisorInput$LogGroupArn`, `^$|^arn:(aws|aws-cn|aws-us-gov):logs:([a-zA-Z0-9-]+):([0-9]+):log-group:[a-zA-Z0-9_\-\/\.]+:\*$`. For it the function strips the `^` that anchored the FIRST branch (`^$`, i.e. "the empty string") and the `$` that terminated the LAST branch, and hands back `$|^arn:(aws|aws-cn|aws-us-gov):logs:...:\*`.

That is not the character class the member must satisfy: read as the unanchored expression the clause presents, its first alternative is the bare end-of-input anchor `$`, which every string satisfies. So a caller who sends `LogGroupArn: "nope"` is told its value failed to satisfy a requirement that the value (and every other value) does satisfy, and the one thing the branch it destroyed encoded — that the EMPTY string is the accepted way to say "no log group", which this crate itself relies on (`HypervisorUpdate::log_group_arn` doc, state.rs:1157-1160) — is no longer stated at all.

The same rule is applied inconsistently elsewhere: for `ImportHypervisorConfigurationInput$KmsKeyArn` (`^(^arn:...$)|(^alias/(\S+)$)$`) the outer pair is stripped while the branches keep their own `^`/`$`, so the clause still reports the anchoring the comment says it removes. The defect is the one-leading/one-trailing assumption in `unanchored`, which this model's alternation patterns break.

## Evidence

The rule the code states for itself — services/service-com-amazonaws-backupgateway-backuponpremises-v20210101/src/service.rs:110-116:

        // The requirement reported is the expression itself, without the `^` / `$` the
        // model spells it with: this service states the character class a value must
        // satisfy, not the anchoring the model uses to make the match total.
        ViolationKind::Pattern(pattern) => format!(
            "Member must satisfy regular expression pattern: {}",
            unanchored(pattern)
        ),

The code under it — src/service.rs:137-153:

    /// A `@pattern` expression without the anchors a model may spell it with. A `$` escaped
    /// by a backslash is a literal dollar sign inside the expression, not the end anchor, so
    /// it stays.
    fn unanchored(pattern: &str) -> &str {
        let body = pattern.strip_prefix('^').unwrap_or(pattern);
        match body.strip_suffix('$') {
            Some(trimmed) => {
                let backslashes = trimmed.len() - trimmed.trim_end_matches('\\').len();
                if backslashes % 2 == 0 {
                    trimmed
                } else {
                    body
                }
            }
            None => body,
        }
    }

The member that breaks it (`smithy_query(query="operation",operation="UpdateHypervisor")`):

    "LogGroupArn": {"target": "LogGroupArn", "length": {"min": 0, "max": 2048},
     "pattern": "^$|^arn:(aws|aws-cn|aws-us-gov):logs:([a-zA-Z0-9-]+):([0-9]+):log-group:[a-zA-Z0-9_\\-\\/\\.]+:\\*$"}

strip_prefix('^') removes the anchor of the branch `^$`; strip_suffix('$') removes the terminator of the second branch; the trailing character is `*`, so `backslashes == 0` and the trimmed form is returned: `$|^arn:(aws|aws-cn|aws-us-gov):logs:([a-zA-Z0-9-]+):([0-9]+):log-group:[a-zA-Z0-9_\-\/\.]+:\*`.

That the empty branch is a value this crate depends on — src/state.rs:1157-1160 (`HypervisorUpdate::log_group_arn`):

    /// The member's pattern is `^$|^arn:…:log-group:…:\*$`, so the EMPTY string is a
    /// value the model accepts and the only way a caller can state "no log group": it
    /// detaches whatever the record carried, which is why an empty value is not the same
    /// as an absent one.

Test coverage: the clause is pinned only for single-branch patterns — tests/all/negative_paths.rs:84, :104, :113, :131, :140, :153 assert e.g. `"Member must satisfy regular expression pattern: [ -~]+"` (Password) and `"… [a-zA-Z0-9-]*"` (Name). No test in the suite asserts the clause for `LogGroupArn` or `KmsKeyArn` (grep for `LogGroupArn` in tests/ shows only success-path round-trips: hypervisor_update.rs:213/234/245/257, hypervisor_host_is_not_a_key.rs:253/277/285, hypervisor_reads.rs:364/425/513), so the alternation rendering is unpinned.

## Steps to Reproduce

1. Start the emulator with the backup-gateway service mounted.
2. Issue an activation key (GET /?activationRegion=us-east-1&no_redirect), CreateGateway, then ImportHypervisorConfiguration to mint a hypervisor and note its HypervisorArn.
3. Call UpdateHypervisor with {"HypervisorArn": <arn>, "LogGroupArn": "nope"} — a value the member's pattern rejects, so the boundary validator refuses it and crate::service::override_refusal words the answer.
4. Read the 400 ValidationException Message: it ends "… failed to satisfy constraint: Member must satisfy regular expression pattern: $|^arn:(aws|aws-cn|aws-us-gov):logs:([a-zA-Z0-9-]+):([0-9]+):log-group:[a-zA-Z0-9_\-\/\.]+:\*" — an expression whose first alternative is the bare `$` anchor, which the rejected value already satisfies, and which no longer states that the empty string is accepted.
5. For contrast, call ImportHypervisorConfiguration with {"Name": "n", "Host": "h.example", "KmsKeyArn": "nope"}: the clause reports "(^arn:…$)|(^alias/(\S+)$)" — i.e. it still carries the `^`/`$` anchoring that service.rs:110-112 says this service does not report.
