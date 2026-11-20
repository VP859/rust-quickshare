//! Tray icon of the background process without GTK: a StatusNotifierItem
//! (the D-Bus protocol KDE, most other desktops and GNOME's AppIndicator
//! extension show in their tray) with a two-item menu (`com.canonical.dbusmenu`).
//!
//! A click on the icon, or "Show window", starts the app (which then stops the
//! background process); "Quit Ferry" stops it.

use super::Stop;
use futures_util::StreamExt;
use rust_i18n::t;
use std::collections::HashMap;
use tokio::sync::mpsc;
use tracing::warn;
use zbus::fdo;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, StructureBuilder, Value};

/// Object path of the item.
const ITEM_PATH: &str = "/StatusNotifierItem";
/// Object path of its menu.
const MENU_PATH: &str = "/MenuBar";
/// The service the tray (the desktop's panel) runs.
const WATCHER: &str = "org.kde.StatusNotifierWatcher";
/// Menu item "Show window".
const SHOW: i32 = 1;
/// Menu separator.
const SEPARATOR: i32 = 2;
/// Menu item "Quit Ferry".
const QUIT: i32 = 3;

/// An icon in the protocol's format: width, height and ARGB32 pixels in
/// network byte order.
type Pixmap = (i32, i32, Vec<u8>);

/// A menu layout: a revision and the item `(id, properties, children)`.
type Layout = (u32, (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>));

/// Proxy of the tray's watcher.
#[allow(missing_docs, clippy::missing_docs_in_private_items)]
mod watcher {
    #[zbus::proxy(
        interface = "org.kde.StatusNotifierWatcher",
        default_service = "org.kde.StatusNotifierWatcher",
        default_path = "/StatusNotifierWatcher"
    )]
    pub trait StatusNotifierWatcher {
        fn register_status_notifier_item(&self, service: &str) -> zbus::Result<()>;
    }
}

/// The app icon in the sizes the tray picks from.
fn pixmaps() -> Vec<Pixmap> {
    [
        &include_bytes!("../../icons/32x32.png")[..],
        &include_bytes!("../../icons/128x128.png")[..],
    ]
    .into_iter()
    .filter_map(|png| tauri::image::Image::from_bytes(png).ok())
    .map(|image| {
        let argb = image
            .rgba()
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|&[r, g, b, a]| [a, r, g, b])
            .collect();
        let size = |n: u32| i32::try_from(n).unwrap_or(0);
        (size(image.width()), size(image.height()), argb)
    })
    .collect()
}

/// The `org.kde.StatusNotifierItem` object.
struct Item {
    /// The icon.
    icon: Vec<Pixmap>,
}

#[zbus::interface(name = "org.kde.StatusNotifierItem")]
impl Item {
    /// The kind of item: an application.
    #[zbus(property)]
    fn category(&self) -> &str {
        "ApplicationStatus"
    }

    /// A stable name of the item.
    #[zbus(property)]
    fn id(&self) -> &str {
        "ferry"
    }

    /// The app's name.
    #[zbus(property)]
    fn title(&self) -> &str {
        "Ferry"
    }

    /// Always shown.
    #[zbus(property)]
    fn status(&self) -> &str {
        "Active"
    }

    /// No icon from the theme: [`Item::icon_pixmap`] is used.
    #[zbus(property)]
    fn icon_name(&self) -> &str {
        ""
    }

    /// The app icon.
    #[zbus(property)]
    fn icon_pixmap(&self) -> Vec<Pixmap> {
        self.icon.clone()
    }

    /// The tooltip: what Ferry does in the background.
    #[zbus(property)]
    fn tool_tip(&self) -> (String, Vec<Pixmap>, String, String) {
        (
            String::new(),
            Vec::new(),
            t!("gui.tray_tooltip").into_owned(),
            String::new(),
        )
    }

    /// A click activates the item; the menu is on the other button.
    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        false
    }

    /// Where the menu is served.
    #[zbus(property)]
    fn menu(&self) -> OwnedObjectPath {
        ObjectPath::from_static_str_unchecked(MENU_PATH).into()
    }

    /// A click on the icon opens the window.
    fn activate(&self, _x: i32, _y: i32) {
        super::open_app();
    }

    /// A middle click, too.
    fn secondary_activate(&self, _x: i32, _y: i32) {
        super::open_app();
    }

    /// Trays that do not show the menu themselves ask for it here; the
    /// window has the same choices.
    fn context_menu(&self, _x: i32, _y: i32) {
        super::open_app();
    }

    /// Scrolling over the icon does nothing.
    fn scroll(&self, _delta: i32, _orientation: &str) {}
}

/// The menu (`com.canonical.dbusmenu`): "Show window", "Quit Ferry".
struct Menu {
    /// Where "Quit Ferry" goes.
    stop: mpsc::UnboundedSender<Stop>,
}

impl Menu {
    /// Properties of the menu item `id`.
    fn properties(id: i32) -> HashMap<String, OwnedValue> {
        let text = |key: &str| OwnedValue::from(zbus::zvariant::Str::from(t!(key).into_owned()));
        let mut props = HashMap::new();
        match id {
            0 => {
                props.insert(
                    "children-display".into(),
                    OwnedValue::from(zbus::zvariant::Str::from_static("submenu")),
                );
            }
            SHOW => {
                props.insert("label".into(), text("gui.tray_show"));
            }
            SEPARATOR => {
                props.insert(
                    "type".into(),
                    OwnedValue::from(zbus::zvariant::Str::from_static("separator")),
                );
            }
            QUIT => {
                props.insert("label".into(), text("gui.tray_quit"));
            }
            _ => {}
        }
        props
    }

    /// A menu item without children, as a layout entry (`(ia{sv}av)` in a variant).
    fn entry(id: i32) -> Option<OwnedValue> {
        let structure = StructureBuilder::new()
            .add_field(id)
            .add_field(Self::properties(id))
            .add_field(Vec::<OwnedValue>::new())
            .build()
            .ok()?;
        OwnedValue::try_from(Value::from(structure)).ok()
    }

    /// Runs the menu item `id`.
    fn clicked(&self, id: i32) {
        match id {
            SHOW => super::open_app(),
            QUIT => {
                let _ = self.stop.send(Stop::Quit);
            }
            _ => {}
        }
    }
}

#[zbus::interface(name = "com.canonical.dbusmenu")]
impl Menu {
    /// The whole menu: it is flat, so the depth asked for does not matter.
    fn get_layout(
        &self,
        parent_id: i32,
        _recursion_depth: i32,
        _property_names: Vec<String>,
    ) -> fdo::Result<Layout> {
        let children = if parent_id == 0 {
            [SHOW, SEPARATOR, QUIT]
                .into_iter()
                .filter_map(Self::entry)
                .collect()
        } else {
            Vec::new()
        };
        Ok((1, (parent_id, Self::properties(parent_id), children)))
    }

    /// Properties of several items.
    fn get_group_properties(
        &self,
        ids: Vec<i32>,
        _property_names: Vec<String>,
    ) -> Vec<(i32, HashMap<String, OwnedValue>)> {
        ids.into_iter()
            .map(|id| (id, Self::properties(id)))
            .collect()
    }

    /// One property of an item.
    fn get_property(&self, id: i32, name: String) -> fdo::Result<OwnedValue> {
        Self::properties(id)
            .remove(&name)
            .ok_or_else(|| fdo::Error::InvalidArgs(name))
    }

    /// A click on an item.
    fn event(&self, id: i32, event_id: String, _data: OwnedValue, _timestamp: u32) {
        if event_id == "clicked" {
            self.clicked(id);
        }
    }

    /// Several events at once; none of the items is unknown.
    fn event_group(&self, events: Vec<(i32, String, OwnedValue, u32)>) -> Vec<i32> {
        for (id, event_id, _, _) in events {
            if event_id == "clicked" {
                self.clicked(id);
            }
        }
        Vec::new()
    }

    /// The menu never changes before it is shown.
    fn about_to_show(&self, _id: i32) -> bool {
        false
    }

    /// See [`Menu::about_to_show`].
    fn about_to_show_group(&self, _ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
        (Vec::new(), Vec::new())
    }

    /// Version of the menu protocol.
    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }

    /// Left to right.
    #[zbus(property)]
    fn text_direction(&self) -> &str {
        "ltr"
    }

    /// Shown normally.
    #[zbus(property)]
    fn status(&self) -> &str {
        "normal"
    }

    /// No icons of its own.
    #[zbus(property)]
    fn icon_theme_path(&self) -> Vec<String> {
        Vec::new()
    }
}

/// Shows the tray icon; it stays while the returned connection lives, and is
/// registered again when the tray restarts (e.g. the panel crashed).
///
/// # Errors
///
/// When there is no session bus or the objects cannot be served.
pub async fn start(stop: mpsc::UnboundedSender<Stop>) -> zbus::Result<zbus::Connection> {
    let name = format!("org.kde.StatusNotifierItem-{}-1", std::process::id());
    let connection = zbus::connection::Builder::session()?
        .name(name.as_str())?
        .serve_at(ITEM_PATH, Item { icon: pixmaps() })?
        .serve_at(MENU_PATH, Menu { stop })?
        .build()
        .await?;
    let watcher = watcher::StatusNotifierWatcherProxy::new(&connection).await?;
    if let Err(e) = watcher.register_status_notifier_item(&name).await {
        // No tray yet: registered when one appears (below).
        warn!("No tray to show the icon in yet: {e}");
    }
    let dbus = fdo::DBusProxy::new(&connection).await?;
    let mut owners = dbus
        .receive_name_owner_changed_with_args(&[(0, WATCHER)])
        .await?;
    tokio::spawn(async move {
        while let Some(change) = owners.next().await {
            let appeared = change.args().is_ok_and(|args| args.new_owner().is_some());
            if appeared && let Err(e) = watcher.register_status_notifier_item(&name).await {
                warn!("Could not show the tray icon: {e}");
            }
        }
    });
    Ok(connection)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every entry is a `(ia{sv}av)` in a variant, as dbusmenu requires, and
    /// the root lists the three items in order.
    #[test]
    fn the_menu_layout_has_the_protocol_types() {
        let entry = Menu::entry(SHOW).expect("an entry");
        assert_eq!(entry.value_signature().to_string(), "(ia{sv}av)");
        let menu = Menu {
            stop: mpsc::unbounded_channel().0,
        };
        let (_, (root, _, children)) = menu.get_layout(0, -1, Vec::new()).expect("a layout");
        assert_eq!(root, 0);
        assert_eq!(children.len(), 3);
        assert!(Menu::properties(QUIT).contains_key("label"));
        assert!(menu.get_property(SEPARATOR, "type".into()).is_ok());
    }

    /// The icon comes in both sizes, as ARGB with four bytes per pixel.
    #[test]
    fn the_icon_is_argb_in_two_sizes() {
        let icons = pixmaps();
        assert_eq!(icons.len(), 2);
        for (width, height, argb) in icons {
            assert_eq!(argb.len(), usize::try_from(width * height * 4).unwrap());
        }
    }
}
