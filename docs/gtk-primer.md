# GTK primer

Notes for working on the GTK app, written for a first GTK project. Each section is filled in as the matching piece of the app lands.

## Where the pieces live

- Windows and pages: composite templates in `crates/mailslot-gtk/resources/ui/`, bound to Rust subclasses.
- Styling: `resources/style.css`, libadwaita named colours only.
- The engine bridge: one place that owns the `Engine` and turns its event stream into list-store updates.

## Template skeleton

A window is a Rust subclass plus an XML template compiled into a gresource.

- The resource prefix is `/ca/liminalhq/mailslot/`, and `#[template(resource = "/ca/liminalhq/mailslot/ui/window.ui")]` must match it.
- Call `klass.bind_template()` in `class_init` and `obj.init_template()` in `instance_init`.
- `#[template_child]` field names must equal the XML `id` values.
- A custom widget used inside a template needs `MyWidget::ensure_type()` before the parent is built.

## Engine events on the main loop

There is no `#[tokio::main]` and no `block_on` on the main thread. Commands go to the engine, and events come back through `glib::spawn_future_local`. Plain data crosses threads. GObjects do not.

## List models

`gio::ListStore` of upload items, `ListBox::bind_model` for rows, and property bindings with transforms for progress fractions.

## QR codes

`qrcode` renders to a `gdk::MemoryTexture` shown in a `gtk::Picture`, with nearest-neighbour scaling and a white quiet zone.

## File dialogs, drops and notifications

`gtk::FileDialog::select_folder` and `gtk::DropTarget` depend on portals. Inside a container, `GDK_DEBUG=no-portals` forces GTK's built-in chooser. Test portal flows natively or in the Flatpak.

`gio::Notification` on GNOME needs a matching installed `.desktop` file, so check notifications with `make run-native` after `make install-dev-desktop`.

## Pitfalls log

Learnings are added here as they are found.
