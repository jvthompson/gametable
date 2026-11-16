//! Admin-started shutdown with a countdown. Port of `shutdown.go`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gametable_core::json::JsonObject;

use super::{Request, Server};
use crate::log;
use crate::payload::sorted_object;
use crate::store::User;

/// How long players get between an admin starting a shutdown and the server closing.
pub(super) const DEFAULT_SHUTDOWN_DELAY: Duration = Duration::from_secs(5 * 60);

impl Server {
    /// Whether a shutdown has been scheduled.
    pub fn is_shutting_down(&self) -> bool {
        self.state().shutdown_at.is_some()
    }

    /// Closes the server early once a shutdown is pending and nobody but the admin who started it
    /// is still online.
    pub(crate) fn shutdown_if_empty(self: &Arc<Self>) {
        let (pending, others_online) = {
            let state = self.state();
            let others = state.client_by_user_id.values().filter(|id| Some(**id) != state.shutdown_started_by).count();
            (state.shutdown_at.is_some(), others)
        };
        if pending && others_online == 0 {
            log!(self, "no players online, shutting down now");
            self.close_in_background();
        }
    }

    fn close_in_background(self: &Arc<Self>) {
        let server = self.clone();
        std::thread::spawn(move || server.close());
    }
}

impl Request<'_> {
    pub(super) fn handle_admin_shutdown(&self, user: &User) {
        let server = self.server;
        let delay = {
            let mut state = server.state();
            if state.shutdown_at.is_some() {
                drop(state);
                self.fail("alreadyShuttingDown", "The server is already shutting down.");
                return;
            }
            let delay = state.shutdown_delay;
            state.shutdown_at = Some(Instant::now() + delay);
            state.shutdown_started_by = Some(self.client_id);
            delay
        };
        let seconds = (delay.as_millis() + 500) / 1000;
        log!(server, "{} started a server shutdown; closing in {}", user.name, go_delay_text(delay));
        self.reply(JsonObject::new().with("seconds", seconds as f64));
        server.broadcast_to_signed_in(
            "serverShutdown",
            sorted_object([("seconds", (seconds as f64).into()), ("by", user.name.as_str().into())]),
        );
        let countdown_server = server.clone();
        let countdown = async move {
            tokio::time::sleep(delay).await;
            log!(countdown_server, "shutdown countdown finished, shutting down");
            countdown_server.close();
        };
        match server.runtime() {
            Some(runtime) => {
                runtime.spawn(countdown);
            }
            None => {
                let server = server.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(delay);
                    server.close();
                });
            }
        }
        server.shutdown_if_empty();
    }
}

/// Go's `time.Duration` text for whole and fractional delays ("5m0s", "300ms").
fn go_delay_text(delay: Duration) -> String {
    if delay < Duration::from_secs(1) { format!("{}ms", delay.as_millis()) } else { super::go_duration_text(delay) }
}
