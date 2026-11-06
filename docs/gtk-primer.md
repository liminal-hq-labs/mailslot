# GTK primer

Notes for working on the GTK app, written for a first GTK project. Each item is something that cost time.

## Where the pieces live

- Windows and pages: composite templates in `crates/mailslot-gtk/resources/ui/`, bound to Rust subclasses.
- Styling: `resources/style.css`, libadwaita named colours only.
- The engine bridge: one place owns the `Engine` and turns its event stream into list-store updates.

## Templates and resources

- The `#[template(resource = "…")]` path is the gresource prefix plus the file path: prefix `/ca/liminalhq/mailslot` and file `ui/window.ui` give `/ca/liminalhq/mailslot/ui/window.ui`. `build.rs` runs `glib_build_tools::compile_resources`, and `gio::resources_register_include!("mailslot.gresource")` must run before the first template is instantiated. Register once, in startup.
- Every `#[template_child]` field name must equal an `id` in the XML. Call `bind_template()` in `class_init` and `init_template()` in `instance_init`.
- If the template XML has an error, GTK prints a `Gtk-CRITICAL: Error building template class …` line, and the panic that follows is the misleading "Failed to retrieve template child. Please check that all fields of type `AdwHeaderBar` have been bound". Read the line above the panic. One cause: `AdwBreakpoint` setters do not accept `<null/>`, so hide a widget with `visible=False` instead.
- A custom widget used inside another template needs `MyWidget::ensure_type()` before the parent is built. Mailslot's row is created from Rust, so it does not need it, but the first `ObjectSubclass::type_()` call is what registers the type.
- `adw::init()` registers the libadwaita types that XML refers to (`AdwToolbarView`, `AdwStatusPage` and so on). Call it before building any template.
- A composite template can subclass `AdwActionRow` and add `type="prefix"` and `type="suffix"` children in the XML. Implement `ListBoxRowImpl`, `PreferencesRowImpl` and `ActionRowImpl` for the subclass.
- Template objects keep their XML `id` as the Buildable id, which makes widget-tree lookups in tests easy: walk `first_child()` and `next_sibling()` and compare `buildable_id()`.

## GObject properties

- `#[derive(glib::Properties)]` with `#[properties(wrapper_type = …)]`, plus `#[glib::derived_properties]` on the `ObjectImpl`, gives typed getters and setters on the wrapper and `notify::` signals. Use `Cell` for `Copy` fields and `RefCell<String>` for strings.
- Use plain strings for small state enums (`receiving`, `done`, `failed`) so they bind and test easily.
- GObjects are not `Send`. Create and touch them on the main thread only.

## Threading

- GLib has no tokio reactor. Polling a future that uses `tokio::fs`, `tokio::time` or tokio networking from `glib::spawn_future_local` panics with "there is no reactor running". The engine owns a tokio runtime, and every command spawns its work on that runtime and awaits the `JoinHandle`, which is executor-agnostic. The same approach makes `engine.subscribe()` (a tokio broadcast stream) safe to poll from the main loop.
- Never `block_on` on the main thread outside tests. In tests, `glib::MainContext::default().block_on(fut)` is fine.
- Consume events with `glib::spawn_future_local`, update `gio::ListStore` models there, and let `ListBox::bind_model` build rows.

## List models

A `gio::ListStore` of upload items feeds `ListBox::bind_model` for rows, with property bindings and transforms for progress fractions.

## API details

- `adw::AlertDialog::choose_future` takes `Option<&impl IsA<gtk::Widget>>`, so pass `Some(&window)`.
- `glib::DateTime::to_local()` returns a `Result`, and `from_iso8601` takes the time zone as an `Option`.
- `activate_action` exists on both `WidgetExt` and `ActionGroupExt`, which makes the call ambiguous on a window. Call `gio::prelude::ActionGroupExt::activate_action(&win, "name", None)` for window-scoped actions.
- `gdk::MemoryTexture::new(…)` returns the concrete type. Import `glib::prelude::*` for `.upcast()`.
- `gettext-rs` 0.7 is flagged by RUSTSEC-2026-0244 because `setlocale` is unsound. Version 0.8 makes it `unsafe`. Call it once at the top of `main`, before any thread exists.
- With `GDK_DEBUG=no-portals`, file dialogs and launchers skip the portals, which is what you want in containers and tests. Test portal flows natively or in the Flatpak.
- A `gio::Notification` without a notification service logs a warning on a bare session bus. That is harmless in tests. On GNOME, notifications need a matching installed desktop file, so check them with `make run-native` after `make install-dev-desktop`.

## Styling

- Use only libadwaita named colours in `style.css`: `@warning_bg_color`, `@error_color`, `@card_shade_color`, `@window_fg_color`. `@borders` is not in the semantic set that `scripts/check-style.sh` accepts.
- Use built-in style classes first (`boxed-list`, `card`, `pill`, `suggested-action`, `destructive-action`, `flat`, `dim-label`). They follow the system accent colour for free.
- Render the QR code dark-on-light into a `MemoryTexture` (`MemoryFormat::G8`) with a four-module quiet zone, and keep the texture around 240 px so `GtkPicture` does not request a huge natural size.

## Background work and discovery

- Decode QR codes off the main thread. Download the texture to plain luminance bytes on the main thread (`gdk::TextureDownloader` with `MemoryFormat::R8g8b8a8`), then hand the `Vec<u8>` to `gio::spawn_blocking`. GObjects never cross threads, and plain bytes do. `rqrr::PreparedImage::prepare_from_greyscale` avoids pulling in the `image` crate.
- `adw::ButtonRow` and extra `AdwPreferencesGroup` children can be added and removed at runtime (`group.add`, `group.remove`), which suits lists of discovered items.
- mDNS announcements and browsing run on their own thread inside `mdns-sd`, and its receivers are `flume` channels whose `recv_async` works from any executor.
- A `GApplication` quits as soon as `activate` returns with no window and no hold. If the window is built after an `await`, take `app.hold()` before spawning the future and keep the guard alive until the window is shown.

## Translations

`make pot` runs `scripts/make-pot.sh`, which extracts strings with `xgettext -L Rust` (GNU gettext 0.24 or newer), `-L Glade` for `.ui` files and the desktop-file rules, and merges them into `po/mailslot.pot`. A file that uses `gettext` must be listed in `po/POTFILES.in`.

## Testing

- GTK must be initialized and used on one thread. Put exactly one `#[test]` in each `ui_*.rs` file, so nextest runs each in its own process, and mark it `#[ignore = "needs display"]`.
- Run under `weston --backend=headless` with a private `XDG_RUNTIME_DIR`, `WAYLAND_DISPLAY` set to the test socket, `GSK_RENDERER=cairo` and `GDK_DEBUG=no-portals`, inside `dbus-run-session`. `scripts/test-ui.sh` does this. Never point tests at the real session.
- A `GtkStack` crossfade is still running 200 ms after the page switch. Let the main loop iterate before taking a screenshot.
- Screenshot a widget with `gtk::WidgetPaintable`, then `Snapshot::to_node()`, then `native().renderer().render_texture()`, then `Texture::save_to_png()`.
- Without the app icon installed in the icon theme, `AdwStatusPage` shows the "image missing" placeholder. Add the icon directory to `XDG_DATA_DIRS` for screenshots.
