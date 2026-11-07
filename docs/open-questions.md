# Open questions

Resolve an item by moving the answer into [decisions.md](decisions.md) and deleting it here.

1. **Repository visibility.** Mailslot lives at `liminal-hq-labs/mailslot`, the first Liminal HQ Labs project. It starts private; make it public when the first release is ready.
2. **Shared dev image.** The Fedora image stays in this repository. Revisit only if another Liminal HQ project needs GTK 4.20 or newer.
3. **Token storage.** The token is kept in the config file (`0600`). Moving it to libsecret is an option, with a cost under Flatpak and on headless machines.
4. **Default bind address.** `0.0.0.0` matches the original script. A LAN-only or localhost default with an explicit expose switch is safer.
5. **Default size cap.** 200 MB matches the original script. 2 GB suits photos and videos better.
6. **Background mode.** Hiding on close, with a Background portal request, is planned after the first release.
7. **Blueprint.** XML templates for now. Revisit when the templates grow heavy.
8. **crates.io.** `mailslot-core` is publish-ready. Publishing happens only when the maintainer decides.
9. **deb floor.** Ubuntu 24.04 is unsupported because of its GTK and libadwaita versions. Recommend the Flatpak there.
10. **Flathub.** Submission after 1.0, once screenshots and release notes exist.
