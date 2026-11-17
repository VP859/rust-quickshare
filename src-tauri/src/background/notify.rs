//! Notifications of the background process, straight over D-Bus
//! (`org.freedesktop.Notifications`): plain ones, and questions with
//! "Accept" and "Decline" buttons.

use futures_util::StreamExt;
use rust_i18n::t;
use std::collections::HashMap;
use std::time::Duration;
use tracing::warn;
use zbus::zvariant::Value;

/// Proxy of the desktop's notification service.
#[allow(missing_docs, clippy::missing_docs_in_private_items)]
mod proxy {
    use std::collections::HashMap;
    use zbus::zvariant::Value;

    #[zbus::proxy(
        interface = "org.freedesktop.Notifications",
        default_service = "org.freedesktop.Notifications",
        default_path = "/org/freedesktop/Notifications"
    )]
    pub trait Notifications {
        #[allow(clippy::too_many_arguments)]
        fn notify(
            &self,
            app_name: &str,
            replaces_id: u32,
            app_icon: &str,
            summary: &str,
            body: &str,
            actions: &[&str],
            hints: HashMap<&str, Value<'_>>,
            expire_timeout: i32,
        ) -> zbus::Result<u32>;

        fn close_notification(&self, id: u32) -> zbus::Result<()>;

        #[zbus(signal)]
        fn action_invoked(&self, id: u32, action_key: String) -> zbus::Result<()>;

        #[zbus(signal)]
        fn notification_closed(&self, id: u32, reason: u32) -> zbus::Result<()>;
    }
}

/// Name of the app in notifications.
const APP_NAME: &str = "Ferry";
/// The app's desktop entry, so the desktop shows its icon and settings.
const DESKTOP_ENTRY: &str = "Ferry";
/// Key of the "Accept" button.
const ACCEPT: &str = "accept";
/// Key of the "Decline" button.
const DECLINE: &str = "decline";

/// Sends notifications; without a notification service (no session bus) it
/// shows nothing and every question is declined.
pub struct Notifier {
    /// The service, when there is one.
    proxy: Option<proxy::NotificationsProxy<'static>>,
}

impl Notifier {
    /// Connects to the notification service of the session.
    pub async fn new() -> Self {
        let proxy = async {
            let connection = zbus::Connection::session().await?;
            proxy::NotificationsProxy::new(&connection).await
        }
        .await
        .inspect_err(|e| warn!("No notifications: {e}"))
        .ok();
        Self { proxy }
    }

    /// Hints of every notification: the desktop entry and the urgency.
    fn hints(urgent: bool) -> HashMap<&'static str, Value<'static>> {
        HashMap::from([
            ("desktop-entry", Value::from(DESKTOP_ENTRY)),
            ("urgency", Value::U8(if urgent { 2 } else { 1 })),
        ])
    }

    /// Shows a notification.
    pub async fn show(&self, summary: &str, body: &str) {
        let Some(proxy) = &self.proxy else {
            return;
        };
        if let Err(e) = proxy
            .notify(APP_NAME, 0, "", summary, body, &[], Self::hints(false), -1)
            .await
        {
            warn!("Could not show a notification: {e}");
        }
    }

    /// Asks with "Accept" and "Decline" buttons; `true` only when "Accept" is
    /// pressed within `timeout`. The notification is removed afterwards.
    pub async fn ask(&self, summary: &str, body: &str, timeout: Duration) -> bool {
        let Some(proxy) = &self.proxy else {
            return false;
        };
        // Subscribed before showing, so a quick answer is not missed.
        let (Ok(mut actions), Ok(mut closed)) = (
            proxy.receive_action_invoked().await,
            proxy.receive_notification_closed().await,
        ) else {
            return false;
        };
        let accept = t!("gui.notify_accept");
        let decline = t!("gui.notify_decline");
        let buttons = [ACCEPT, &accept, DECLINE, &decline];
        // Never expires by itself: the timeout below removes it.
        let id = match proxy
            .notify(
                APP_NAME,
                0,
                "",
                summary,
                body,
                &buttons,
                Self::hints(true),
                0,
            )
            .await
        {
            Ok(id) => id,
            Err(e) => {
                warn!("Could not ask in a notification: {e}");
                return false;
            }
        };
        let answer = async {
            loop {
                tokio::select! {
                    Some(signal) = actions.next() => {
                        if let Ok(args) = signal.args()
                            && *args.id() == id
                        {
                            return args.action_key() == ACCEPT;
                        }
                    }
                    Some(signal) = closed.next() => {
                        if signal.args().is_ok_and(|args| *args.id() == id) {
                            return false;
                        }
                    }
                    else => return false,
                }
            }
        };
        let accepted = tokio::time::timeout(timeout, answer).await.unwrap_or(false);
        let _ = proxy.close_notification(id).await;
        accepted
    }
}
