Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 9ff0151e93167cf00d59909edc96bc1d4e3529be

# ARN minting for the two resource types is the same computation written twice in src/state.rs

_Filed: 2026-10-25T10:48:37Z_
_Operation: CreateDashboard | Class: maintainability_

> _Note: this issue was identified against the code base at commit `9ff0151e93167cf00d59909edc96bc1d4e3529be`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "mint a fresh resource ARN" — bump this resource type's monotonic counter, render it as 32 lowercase hex characters, and build the ARN through `engine::emulator_arn` with the `bcm-dashboards` service and the empty region the model's ARN patterns require — is implemented twice in this crate's own state module, once per resource type. `mint_scheduled_report_arn`'s own doc comment admits it is a second copy ("the same way dashboard ARNs are minted"). The two copies can drift: a change to the id width, the region argument, the counter-overflow handling (`wrapping_add`) or the `emulator_arn` call in one of them silently leaves the other producing a differently-shaped ARN, and the resource-id rendering is a third copy in `mint_widget_id`. The fix is internal: one private helper taking the counter and the ARN resource-type segment, called by both minters — it mints byte-identical ARNs, so every response and every existing test stays exactly as it is.

## Evidence

Copy 1 — services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/src/state.rs:356-360:
```
    pub fn mint_dashboard_arn(&mut self) -> String {
        self.next_dashboard_seq = self.next_dashboard_seq.wrapping_add(1);
        let resource_id = format!("{:032x}", self.next_dashboard_seq);
        ::engine::emulator_arn("bcm-dashboards", "", &format!("dashboard/{resource_id}"))
    }
```
Copy 2 — src/state.rs:436-444, whose doc at src/state.rs:432-433 states "Mint a fresh, unique scheduled-report ARN, the same way dashboard ARNs are minted":
```
    pub fn mint_scheduled_report_arn(&mut self) -> String {
        self.next_scheduled_report_seq = self.next_scheduled_report_seq.wrapping_add(1);
        let resource_id = format!("{:032x}", self.next_scheduled_report_seq);
        ::engine::emulator_arn(
            "bcm-dashboards",
            "",
            &format!("scheduled-report/{resource_id}"),
        )
    }
```
The resource-id rendering appears a third time at src/state.rs:427-430 (`mint_widget_id`: `format!("{:032x}", self.next_widget_seq)`).

Callers: src/ops/create_dashboard.rs:242 (`let arn = state.mint_dashboard_arn();`) and src/ops/create_scheduled_report.rs:263 (`let arn = state.mint_scheduled_report_arn();`).

Existing test coverage that must stay unchanged (both pin the minted ARN against the modelled pattern, checked by hand): tests/all/dashboard_lifecycle.rs:101-129 (`fn is_dashboard_arn`) applied to `CreateDashboard`'s output, and tests/all/scheduled_report_lifecycle.rs:40-68 (`fn is_scheduled_report_arn`) applied to `CreateScheduledReport`'s output.

## Steps to Reproduce

1. Open services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/src/state.rs and read `mint_dashboard_arn` (lines 356-360) and `mint_scheduled_report_arn` (lines 436-444): the two bodies differ only in which counter field they bump and in the ARN resource-type segment string.
2. Note src/state.rs:432-433, where the second minter's doc comment states it mints "the same way dashboard ARNs are minted" — the duplication is acknowledged in prose instead of being shared in code.
3. Confirm the id rendering is duplicated a third time in `mint_widget_id` (src/state.rs:427-430).
4. Verify a behaviour-preserving fix is possible: a single private helper `fn mint_arn(counter: &mut u64, segment: &str) -> String` bumping the passed counter and calling `engine::emulator_arn("bcm-dashboards", "", &format!("{segment}/{id}"))` reproduces both ARNs byte for byte, so tests/all/dashboard_lifecycle.rs and tests/all/scheduled_report_lifecycle.rs keep passing untouched.
