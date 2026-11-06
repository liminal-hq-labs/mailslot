# GTK 4 pitfalls hit while building Mailslot

Notes for `docs/gtk-primer.md`. Each item is something that cost time.

## Templates and resources

- The `#[template(resource = "...")]` path must equal the gresource prefix plus the file path: prefix `/ca/liminalhq/mailslot` and file `ui/window.ui` give `/ca/liminalhq/mailslot/ui/window.ui`. `build.rs` runs `glib_build_tools::compile_resources`, and `gio::resources_register_include!("mailslot.gresource")` must run before the first template is instantiated (register once, in startup).
- If the template XML has an error, GTK prints a `Gtk-CRITICAL: Error building template class …` line and the panic you see is the misleading "Failed to retrieve template child. Please check that all fields of type `AdwHeaderBar` have been bound". Read the line above the panic. Our case: `AdwBreakpoint` setters do not accept `<null/>`; hide a widget with `visible=False` instead.
- Every `#[template_child]` field name must equal an `id` in the XML. `bind_template()` goes in `class_init` and `init_template()` in `instance_init`.
- A custom widget used inside another template needs `MyWidget::ensure_type()` before the parent is built. Mailslot's row is created from Rust, so it did not need it, but the first `ObjectSubclass::type_()` call is what registers the type.
- `adw::init()` registers the libadwaita types that XML refers to (`AdwToolbarView`, `AdwStatusPage`, …). Call it before building any template.
- Composite templates can subclass `AdwActionRow` and add `type="prefix"` and `type="suffix"` children in the XML. Implement `ListBoxRowImpl`, `PreferencesRowImpl` and `ActionRowImpl` for the subclass.
- Template objects keep their XML `id` as the Buildable id, which makes widget-tree lookups in tests easy: `buildable_id()` while walking `first_child()` and `next_sibling()`.

## GObject properties

- `#[derive(glib::Properties)]` with `#[properties(wrapper_type = …)]` plus `#[glib::derived_properties]` on the `ObjectImpl` gives typed getters and setters on the wrapper and `notify::` signals. Cells for `Copy` fields, `RefCell<String>` for strings.
- Use plain strings for small state enums (`receiving`, `done`, `failed`) so they bind and test easily.
- GObjects are not `Send`. Create and touch them on the main thread only.

## Threading

- GLib has no tokio reactor. Polling a future that uses `tokio::fs`, `tokio::time` or tokio networking from `glib::spawn_future_local` panics ("there is no reactor running"). The engine therefore owns a tokio runtime and every command spawns its work on that runtime, then awaits the `JoinHandle`, which is executor-agnostic. The same trick makes `engine.subscribe()` (a tokio broadcast stream) safe to poll from the main loop.
- Never `block_on` on the main thread outside tests. In tests, `glib::MainContext::default().block_on(fut)` is fine.
- Consume events with `glib::spawn_future_local`, update `gio::ListStore` models there, and let `ListBox::bind_model` build rows.

## API details

- `adw::AlertDialog::choose_future` takes `Option<&impl IsA<gtk::Widget>>`, so pass `Some(&window)`.
- `glib::DateTime::to_local()` returns a `Result`; `from_iso8601` takes the time zone as an `Option`.
- `activate_action` exists on both `WidgetExt` and `ActionGroupExt`, which makes the call ambiguous on a window. Call `gio::prelude::ActionGroupExt::activate_action(&win, "name", None)` for window-scoped actions.
- `gdk::MemoryTexture::new(...)` returns the concrete type; import `glib::prelude::*` for `.upcast()`.
- `gettext-rs` 0.7 is flagged by RUSTSEC-2026-0244 because `setlocale` is unsound. 0.8 makes it `unsafe`; call it once at the top of `main` before any thread exists.
- With `GDK_DEBUG=no-portals`, file dialogs and launchers skip the portals, which is what you want in containers and tests.
- A `gio::Notification` without a notification service logs a warning on a bare session bus; harmless in tests.

## Styling

- Only libadwaita named colours in `style.css`: `@warning_bg_color`, `@error_color`, `@card_shade_color`, `@window_fg_color`. `@borders` is not in the semantic set that `scripts/check-style.sh` accepts.
- Use built-in style classes first (`boxed-list`, `card`, `pill`, `suggested-action`, `destructive-action`, `flat`, `dim-label`); they follow the system accent colour for free.
- Render the QR code dark-on-light into a `MemoryTexture` (`MemoryFormat::G8`) with a four-module quiet zone, and keep the texture around 240 px so `GtkPicture` does not request a huge natural size.

## Testing

- GTK must be initialized and used on one thread. Put exactly one `#[test]` in each `ui_*.rs` file, so nextest runs each in its own process, and mark it `#[ignore = "needs display"]`.
- Run under `weston --backend=headless` with a private `XDG_RUNTIME_DIR`, `WAYLAND_DISPLAY` set to the test socket, `GSK_RENDERER=cairo`, `GDK_DEBUG=no-portals` and inside `dbus-run-session`. Never point tests at the real session.
- A `GtkStack` crossfade is still running 200 ms after the page switch; let the main loop iterate before taking a screenshot.
- Screenshot a widget with `gtk::WidgetPaintable` → `Snapshot::to_node()` → `native().renderer().render_texture()` → `Texture::save_to_png()`.
- Without the app icon installed in the icon theme, `AdwStatusPage` shows the "image missing" placeholder; add the icon directory to `XDG_DATA_DIRS` for screenshots.
