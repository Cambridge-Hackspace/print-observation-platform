# Testing

One command runs everything: `tools/check.sh`. It needs only podman (or
docker, with `CONTAINER=docker`), because every tool comes from an image
pinned by digest in `tools/images.sh`. CI runs it, and so does the reaper
battery (`reaper test`, configured in `.reaper.toml`), so a green check means
the same thing in both places.

Each suite is there for defects the cheaper ones cannot see.

| Suite | Runs | Catches |
| --- | --- | --- |
| `cargo fmt`, `cargo clippy -D warnings` | `tools/rust.sh` | Style drift and the mistakes clippy knows. |
| Unit tests, `crates/*/src` | `tools/rust.sh` | Configuration: defaults, and errors that name the setting at fault. |
| Server tests, `crates/pop-server/tests/server.rs` | `tools/rust.sh` | The router against a real SQLite database: health, status, the web app's routing rules (an API path never gets the app's HTML; hashed files cache for good, others don't; a missing file is a 404), and the live stream (a snapshot on connect; events in order to every screen; a screen that drops and resumes ending where a steady one does, sent every event once; a snapshot for a resume too late, from another server, or a screen that fell behind; and every stream ending on shutdown). |
| Type drift, `crates/pop-server/tests/types.rs` | `tools/rust.sh` | The web app's generated types no longer matching the server's. |
| `svelte-check`, Vitest, `web/src/**/*.test.ts` | `tools/web/build.sh check` | Type errors in the app, and the live-update client: what counts as a valid event; reconnecting after the browser gives up, waiting longer each time up to a limit, and from the last event seen; and a screen's state as the fold of its stream. |
| Design system, `web/src/lib/design/design.test.ts` | `tools/web/build.sh check` | A drawing drifting from the approved round 6 look: every printer, spool and forest piece is compared with the kit's own output (`docs/design/round-6`), element by element and attribute by attribute; the tokens with its palette; numbers written as its Python writes them; progress said to a screen reader and still for reduced motion; status never by color alone. |
| Playwright, `web/e2e` | `tools/web/e2e.sh` | The wiring no other suite can see: a release binary serving the app it was built with, the app going live over the stream, and showing what the API reports; Lexend bundled, with nothing fetched from outside; and an accessibility check (axe, WCAG 2 AA, color contrast on the dark palette included) of the app and the design system. |

Every assertion was checked by breaking the code it covers and watching it
fail, then restoring it.

## What is not proven

- **Live updates through a reverse proxy.** A proxy that buffers responses
  holds Server-Sent Events back; deployment will need its own check.
- **The kiosk's hardware.** Chromium in `cage` on the tower's box (ADR 0001)
  is not part of any suite yet.
