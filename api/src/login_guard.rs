/// Guards against Dualis account lockout (Dualis locks the account after
/// 10 failed logins).
///
/// Every login attempt goes through `LoginGuard::run`. Consecutive failed
/// logins are counted; once the first failure has been followed by
/// `max_retries` further failures, all login attempts are refused without
/// contacting Dualis. A successful login resets the counter.
///
/// The counter is persisted to a JSON file so it survives process and
/// container restarts (mount the file's directory as a volume in Docker).
///
/// The lock is lifted when:
///   - the configured credentials change (detected via a fingerprint), or
///   - the state file is deleted.
use std::future::Future;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tracing::{error, info, warn};

use crate::error::AppError;

#[derive(Debug, Default, Serialize, Deserialize)]
struct GuardState {
    /// Number of consecutive failed logins.
    failed_attempts: u32,
    /// SHA-256 of the credentials the failures were counted for.
    credentials_fingerprint: String,
    last_failure_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
}

pub struct LoginGuard {
    state: Mutex<GuardState>,
    path: PathBuf,
    max_retries: u32,
}

impl LoginGuard {
    /// Load persisted state from `path`. A missing file means a clean state;
    /// an unreadable or corrupt file is an error, so we never silently forget
    /// earlier failures.
    pub fn load(
        path: PathBuf,
        max_retries: u32,
        username: &str,
        password: &str,
    ) -> Result<Self, String> {
        let fingerprint = credentials_fingerprint(username, password);

        let mut state = match std::fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str::<GuardState>(&raw).map_err(|e| {
                format!(
                    "Login state file {} is corrupt ({e}). Delete it to reset the login counter.",
                    path.display()
                )
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => GuardState::default(),
            Err(e) => {
                return Err(format!(
                    "Could not read login state file {}: {e}",
                    path.display()
                ))
            }
        };

        if state.credentials_fingerprint != fingerprint {
            if state.failed_attempts > 0 {
                info!(
                    previous_failures = state.failed_attempts,
                    "Credentials changed, resetting failed login counter"
                );
            }
            state = GuardState {
                credentials_fingerprint: fingerprint,
                ..GuardState::default()
            };
        }

        let guard = Self {
            state: Mutex::new(state),
            path,
            max_retries,
        };

        // Make sure the file is writable now rather than finding out after
        // the first failed login.
        guard.persist_blocking()?;

        Ok(guard)
    }

    /// Log the current state at startup.
    pub async fn log_status(&self) {
        let state = self.state.lock().await;
        if self.is_locked(&state) {
            error!(
                failed_attempts = state.failed_attempts,
                last_error = ?state.last_error,
                path = %self.path.display(),
                "Dualis login is LOCKED. Fix DUALIS_USERNAME/DUALIS_PASSWORD or delete the state file to retry."
            );
        } else if state.failed_attempts > 0 {
            warn!(
                failed_attempts = state.failed_attempts,
                remaining = self.remaining(&state),
                "Previous failed Dualis logins recorded"
            );
        }
    }

    /// Run a login attempt, unless the retry budget is exhausted.
    ///
    /// Logins are serialized so concurrent requests can't slip past the
    /// counter. Only `AppError::LoginFailed` counts as a failed attempt;
    /// network errors don't reach Dualis' credential check reliably and are
    /// passed through without touching the counter.
    pub async fn run<T, F, Fut>(&self, login: F) -> Result<T, AppError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, AppError>>,
    {
        let mut state = self.state.lock().await;

        if self.is_locked(&state) {
            return Err(AppError::LoginLocked(format!(
                "Dualis login disabled after {} consecutive failed attempts (last error: {}). \
                 Fix DUALIS_USERNAME/DUALIS_PASSWORD or delete {} to retry.",
                state.failed_attempts,
                state.last_error.as_deref().unwrap_or("unknown"),
                self.path.display(),
            )));
        }

        let result = login().await;

        match &result {
            Ok(_) => {
                if state.failed_attempts > 0 {
                    info!(
                        previous_failures = state.failed_attempts,
                        "Login succeeded, resetting failed login counter"
                    );
                    state.failed_attempts = 0;
                    state.last_error = None;
                    state.last_failure_at = None;
                    self.persist(&state);
                }
            }
            Err(AppError::LoginFailed(msg)) => {
                state.failed_attempts += 1;
                state.last_error = Some(msg.clone());
                state.last_failure_at = Some(Utc::now());
                self.persist(&state);

                if self.is_locked(&state) {
                    error!(
                        failed_attempts = state.failed_attempts,
                        "Dualis login failed, no retries left. Further logins are blocked."
                    );
                } else {
                    warn!(
                        failed_attempts = state.failed_attempts,
                        remaining = self.remaining(&state),
                        "Dualis login failed"
                    );
                }
            }
            Err(_) => {}
        }

        result
    }

    fn is_locked(&self, state: &GuardState) -> bool {
        state.failed_attempts > self.max_retries
    }

    fn remaining(&self, state: &GuardState) -> u32 {
        (self.max_retries + 1).saturating_sub(state.failed_attempts)
    }

    fn persist(&self, state: &GuardState) {
        if let Err(e) = write_state(&self.path, state) {
            // The in-memory counter still protects us until the next restart.
            error!(path = %self.path.display(), "Failed to persist login state: {e}");
        }
    }

    fn persist_blocking(&self) -> Result<(), String> {
        let state = self.state.try_lock().map_err(|e| e.to_string())?;
        write_state(&self.path, &state)
            .map_err(|e| format!("Could not write login state file {}: {e}", self.path.display()))
    }
}

/// Write atomically (temp file + rename) so a crash can't leave a half-written file.
fn write_state(path: &Path, state: &GuardState) -> std::io::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(state)?)?;
    std::fs::rename(&tmp, path)
}

fn credentials_fingerprint(username: &str, password: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(username.as_bytes());
    hasher.update([0]);
    hasher.update(password.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("login-guard-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("login-state.json")
    }

    async fn fail(guard: &LoginGuard) -> Result<(), AppError> {
        guard
            .run(|| async { Err::<(), _>(AppError::LoginFailed("bad".into())) })
            .await
    }

    #[tokio::test]
    async fn locks_after_max_retries_and_persists() {
        let path = temp_path("locks");
        let guard = LoginGuard::load(path.clone(), 2, "user", "pw").unwrap();

        for _ in 0..3 {
            assert!(matches!(fail(&guard).await, Err(AppError::LoginFailed(_))));
        }
        assert!(matches!(fail(&guard).await, Err(AppError::LoginLocked(_))));

        // Survives a restart.
        let guard = LoginGuard::load(path.clone(), 2, "user", "pw").unwrap();
        assert!(matches!(fail(&guard).await, Err(AppError::LoginLocked(_))));

        // Changing credentials resets.
        let guard = LoginGuard::load(path, 2, "user", "new-pw").unwrap();
        assert!(matches!(fail(&guard).await, Err(AppError::LoginFailed(_))));
    }

    #[tokio::test]
    async fn success_resets_counter() {
        let path = temp_path("reset");
        let guard = LoginGuard::load(path, 2, "user", "pw").unwrap();

        fail(&guard).await.ok();
        fail(&guard).await.ok();
        guard.run(|| async { Ok::<_, AppError>(()) }).await.unwrap();
        for _ in 0..3 {
            assert!(matches!(fail(&guard).await, Err(AppError::LoginFailed(_))));
        }
        assert!(matches!(fail(&guard).await, Err(AppError::LoginLocked(_))));
    }

    #[tokio::test]
    async fn network_errors_do_not_count() {
        let path = temp_path("network");
        let guard = LoginGuard::load(path, 0, "user", "pw").unwrap();

        for _ in 0..5 {
            let r = guard
                .run(|| async { Err::<(), _>(AppError::Parse("network".into())) })
                .await;
            assert!(matches!(r, Err(AppError::Parse(_))));
        }
        assert!(matches!(fail(&guard).await, Err(AppError::LoginFailed(_))));
        assert!(matches!(fail(&guard).await, Err(AppError::LoginLocked(_))));
    }
}
