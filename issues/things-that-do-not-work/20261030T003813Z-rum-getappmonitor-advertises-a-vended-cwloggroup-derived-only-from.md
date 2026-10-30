Issue-Source: trace-sweep-divergence
Issue-Trace: rum-app-monitor-full-configuration
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rum-app-monitor-full-configuration rum-basic-app-monitor-lifecycle
Issue-Shape-Id: com.amazonaws.rum#RUM
Issue-Key: rum|GetAppMonitor|2xx->2xx|$.AppMonitor.DataStorage.CwLog.CwLogGroup
Issue-Commit: 2e599acd334caf8a9285370c1c224b290eb763d0

# GetAppMonitor advertises a vended CwLogGroup derived only from the monitor name, not the group RUM actually vends

> _Note: this issue was identified against the code base at commit `2e599acd334caf8a9285370c1c224b290eb763d0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAppMonitor

## Description
Cause 1 of 1: GetAppMonitor reports a DataStorage.CwLog.CwLogGroup that this emulator synthesises from the app monitor name alone, so the name it advertises is not the vended log-group name the recorded service reports for the same monitor.

AWS: recorded behavior only. Smithy (services/service-com-amazonaws-rum-rum/smithy-model/com-amazonaws-rum-rum.json) documents CwLog$CwLogGroup only as "The name of the log group where the copies are stored" and places no pattern or length on it, and GetAppMonitor models no error for this path (200 with AppMonitor). The recording of a monitor created with CwLogEnabled:true answers 200 and carries DataStorage.CwLog.CwLogGroup as a value the fixture had to capture as a trace variable (cw_log_group) rather than as text derivable from the request: the vended group name RUM reports is `/aws/vendedlogs/RUMService_<appMonitorName>` PLUS a suffix the service itself chooses per monitor. The fixed part of the value is the `/aws/vendedlogs/RUMService_` prefix; the variable parts are the monitor name from the request and that service-chosen suffix, which is state, not a request value. The emulator's own source states the same thing: `services/service-com-amazonaws-rum-rum/src/state.rs:115-120` says "The suffix AWS appends is not in the model, so this emulator derives it from the monitor's own (account-unique) name".

Emulator: the replayed GetAppMonitor answers 200 but with DataStorage.CwLog.CwLogGroup = `/aws/vendedlogs/RUMService_<appMonitorName>` with no suffix at all — the prefix concatenated with the name and nothing else (`cw_log_group_name` at state.rs:2334-2336, reached from `AppMonitorRecord::cw_log_group` at state.rs:926-932). Replay reports `$.AppMonitor.DataStorage.CwLog.CwLogGroup: value_mismatch (expected the group the trace captured, got '/aws/vendedlogs/RUMService_<name>')` on every GetAppMonitor of a delivery-enabled monitor; the same projection is the one Create/Update use to PROVISION the group, so the name is wrong consistently rather than only on read.

Why: a client that reads CwLogGroup and then uses it against CloudWatch Logs — tailing it, setting retention, reading its resource policy, or asserting on it in infrastructure code — is handed a name that is a function of the app monitor name only. Any client or test that learned the group name from a real RUM response (or that expects the name not to be reconstructible from the monitor name) resolves a different group, and a caller that round-trips the recorded name back into this emulator addresses a group that does not exist. It also makes the group name collide with any other monitor of the same name across accounts, which the real vended-group suffix exists to prevent.

Fix: the app monitor record's CwLog projection in `services/service-com-amazonaws-rum-rum/src/state.rs` (`cw_log_group_name` / `AppMonitorRecord::cw_log_group`) and the delivery provisioner in `src/state/cw_log_delivery.rs` that must create the SAME group: the vended group name has to be minted once per monitor and stored on the record (so reads are stable and the advertised name equals the provisioned one) instead of being recomputed from the name on every read. Hypothesis: the recorded name carries a service-chosen suffix beyond the monitor name, which is why the fixture had to capture it as a variable; only the prefix and the embedded monitor name are requirements, the suffix's literal text from any one recording is not.

Related but NOT covering this: open issue 20261029T072756Z-rum-createappmonitor-event-propagation.md concerns a refused create leaving `/aws/vendedlogs/RUMService_<name>` behind in CloudWatch Logs; it is about the ordering of that side effect, not about the name GetAppMonitor advertises. No open issue carries this replay key.

## Evidence
Fixture rum-app-monitor-full-configuration, step task#2 (GetAppMonitor), expected status 200, actual status 200:
  $.AppMonitor.DataStorage.CwLog.CwLogGroup: value_mismatch (expected '{{cw_log_group}}' — the group name the trace captured from the recorded response — got '/aws/vendedlogs/RUMService_sim2real-probe-rum-rum-fullcfg-d1764408')

Same cluster, 2 affected steps: rum-app-monitor-full-configuration#task#2 and rum-basic-app-monitor-lifecycle#task#4, i.e. every GetAppMonitor in the corpus that reads a monitor with CwLogEnabled:true.

Emulator source confirming the derivation (no stored, service-minted name):
  state.rs:115-120  CW_LOG_GROUP_PREFIX = "/aws/vendedlogs/RUMService_", with the comment "The suffix AWS appends is not in the model, so this emulator derives it from the monitor's own (account-unique) name"
  state.rs:2334-2336  cw_log_group_name(name) = format!("{CW_LOG_GROUP_PREFIX}{name}")
  state.rs:926-932  AppMonitorRecord::cw_log_group() -> that name whenever cw_log_enabled

Note the suffix's literal characters from any single recording are not the requirement; the requirement is that the advertised name is the group RUM vended for that monitor (prefix + monitor name + the service-chosen suffix held in state) and that the same name is the one provisioned in CloudWatch Logs.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Fixture rum-app-monitor-full-configuration, step task#2: create an app monitor with a full configuration including CwLogEnabled:true, then GET /appmonitor/{Name}.
3. Read AppMonitor.DataStorage.CwLog.CwLogGroup in the 200 response: it is exactly '/aws/vendedlogs/RUMService_' + the request's Name.
4. Compare with the recorded response, whose CwLogGroup the fixture had to capture as a variable because it is not reconstructible from the request; the two differ, and the step is reported as a value_mismatch on that path.
5. Same divergence via rum-basic-app-monitor-lifecycle#task#4 (GetAppMonitor after a create with log delivery on).
