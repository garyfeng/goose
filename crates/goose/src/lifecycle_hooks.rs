use crate::config::Config;
use chrono::Utc;
use serde::Serialize;
use std::io::Write;
use std::path::PathBuf;

#[derive(Clone)]
pub struct TurnLifecycleContext {
    pub platform: String,
    pub session_id: String,
    pub session_ref: String,
    pub working_dir: PathBuf,
    pub turn_id: String,
}

#[derive(Clone)]
pub struct TurnLifecycleGuard {
    context: TurnLifecycleContext,
    ended: bool,
}

impl TurnLifecycleGuard {
    pub fn start(context: TurnLifecycleContext) -> Self {
        emit_event(&LifecyclePayload::start(&context));
        Self {
            context,
            ended: false,
        }
    }

    pub fn finish_success(&mut self) {
        self.finish(LifecycleOutcome::Success, None);
    }

    pub fn finish_error(&mut self, error_type: Option<String>) {
        self.finish(LifecycleOutcome::Error, error_type);
    }

    pub fn finish_cancelled(&mut self) {
        self.finish(LifecycleOutcome::Cancelled, None);
    }

    fn finish(&mut self, outcome: LifecycleOutcome, error_type: Option<String>) {
        if self.ended {
            return;
        }
        emit_event(&LifecyclePayload::end(&self.context, outcome, error_type));
        self.ended = true;
    }
}

impl Drop for TurnLifecycleGuard {
    fn drop(&mut self) {
        if self.ended {
            return;
        }

        self.finish_success();
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
enum LifecycleEvent {
    TurnStart,
    TurnEnd,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum LifecycleOutcome {
    Success,
    Error,
    Cancelled,
}

#[derive(Serialize)]
struct LifecyclePayload {
    version: u8,
    event: LifecycleEvent,
    timestamp: String,
    platform: String,
    session_id: String,
    session_ref: String,
    working_dir: String,
    turn_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<LifecycleOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_type: Option<String>,
}

impl LifecyclePayload {
    fn start(context: &TurnLifecycleContext) -> Self {
        Self {
            version: 1,
            event: LifecycleEvent::TurnStart,
            timestamp: Utc::now().to_rfc3339(),
            platform: context.platform.clone(),
            session_id: context.session_id.clone(),
            session_ref: context.session_ref.clone(),
            working_dir: context.working_dir.display().to_string(),
            turn_id: context.turn_id.clone(),
            outcome: None,
            error_type: None,
        }
    }

    fn end(
        context: &TurnLifecycleContext,
        outcome: LifecycleOutcome,
        error_type: Option<String>,
    ) -> Self {
        Self {
            version: 1,
            event: LifecycleEvent::TurnEnd,
            timestamp: Utc::now().to_rfc3339(),
            platform: context.platform.clone(),
            session_id: context.session_id.clone(),
            session_ref: context.session_ref.clone(),
            working_dir: context.working_dir.display().to_string(),
            turn_id: context.turn_id.clone(),
            outcome: Some(outcome),
            error_type,
        }
    }
}

fn event_name(event: &LifecycleEvent) -> &'static str {
    match event {
        LifecycleEvent::TurnStart => "turn-start",
        LifecycleEvent::TurnEnd => "turn-end",
    }
}

fn emit_event(payload: &LifecyclePayload) {
    let Ok(hook) = Config::global().get_param::<String>("GOOSE_LIFECYCLE_HOOK") else {
        return;
    };

    let Ok(input_json) = serde_json::to_vec(payload) else {
        return;
    };

    let event_name = event_name(&payload.event).to_string();

    std::thread::spawn(move || {
        #[cfg(target_os = "windows")]
        let child_result = std::process::Command::new("cmd")
            .arg("/C")
            .arg(format!("{} {}", hook, event_name))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();

        #[cfg(not(target_os = "windows"))]
        let child_result = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("{} {}", hook, event_name))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();

        let Ok(mut child) = child_result else {
            return;
        };

        if let Some(stdin) = child.stdin.as_mut() {
            let _ = stdin.write_all(&input_json);
        }

        let _ = child.wait();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::fs;
    use std::time::{Duration, Instant};
    use tempfile::tempdir;

    fn sample_context() -> TurnLifecycleContext {
        TurnLifecycleContext {
            platform: "goose-cli".to_string(),
            session_id: "session-123".to_string(),
            session_ref: "goose://session/session-123".to_string(),
            working_dir: PathBuf::from("/tmp/project"),
            turn_id: "turn-abc".to_string(),
        }
    }

    #[test]
    fn lifecycle_payload_start_has_expected_fields() {
        let ctx = sample_context();
        let payload = LifecyclePayload::start(&ctx);

        assert_eq!(payload.version, 1);
        assert!(matches!(payload.event, LifecycleEvent::TurnStart));
        assert_eq!(payload.platform, "goose-cli");
        assert_eq!(payload.session_id, "session-123");
        assert_eq!(payload.session_ref, "goose://session/session-123");
        assert_eq!(payload.working_dir, "/tmp/project");
        assert_eq!(payload.turn_id, "turn-abc");
        assert!(payload.outcome.is_none());
        assert!(payload.error_type.is_none());
    }

    #[test]
    fn lifecycle_payload_end_has_outcome_and_error_type() {
        let ctx = sample_context();
        let payload = LifecyclePayload::end(
            &ctx,
            LifecycleOutcome::Error,
            Some("reply_internal_failed".to_string()),
        );

        assert_eq!(payload.version, 1);
        assert!(matches!(payload.event, LifecycleEvent::TurnEnd));
        assert!(matches!(payload.outcome, Some(LifecycleOutcome::Error)));
        assert_eq!(payload.error_type.as_deref(), Some("reply_internal_failed"));
    }

    #[test]
    fn event_name_mapping_matches_contract() {
        assert_eq!(event_name(&LifecycleEvent::TurnStart), "turn-start");
        assert_eq!(event_name(&LifecycleEvent::TurnEnd), "turn-end");
    }

    #[test]
    fn turn_lifecycle_guard_emits_end_once() {
        let ctx = sample_context();
        let mut guard = TurnLifecycleGuard {
            context: ctx,
            ended: false,
        };

        guard.finish_success();
        assert!(guard.ended);

        guard.finish_error(Some("should_not_override".to_string()));
        assert!(guard.ended);
    }

    #[test]
    #[serial]
    fn emit_event_invokes_hook_with_expected_events_and_payload() {
        let dir = tempdir().expect("tempdir");
        let hook_log = dir.path().join("hook.log");
        let hook_script = dir.path().join("hook.sh");

        let script = format!(
            "#!/usr/bin/env bash\nset -euo pipefail\ncat >> \"{}\"\n",
            hook_log.display()
        );
        fs::write(&hook_script, script).expect("write hook script");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&hook_script).expect("metadata").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&hook_script, perms).expect("chmod");
        }

        std::env::set_var(
            "GOOSE_LIFECYCLE_HOOK",
            hook_script.to_string_lossy().to_string(),
        );

        let context = sample_context();

        {
            let mut guard = TurnLifecycleGuard::start(context);
            guard.finish_success();
        }

        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(2) {
            if let Ok(content) = fs::read_to_string(&hook_log) {
                if content.contains("\"event\":\"turn-start\"")
                    && content.contains("\"event\":\"turn-end\"")
                    && content.contains("\"outcome\":\"success\"")
                {
                    std::env::remove_var("GOOSE_LIFECYCLE_HOOK");
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        let content = fs::read_to_string(&hook_log).unwrap_or_default();
        std::env::remove_var("GOOSE_LIFECYCLE_HOOK");
        panic!("expected lifecycle events not found in hook output: {content}");
    }
}
