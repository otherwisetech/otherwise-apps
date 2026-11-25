# otherwise-apps — for agents and contributors

Same conventions as genealogy-rs, diskmap, rama and mosaico.

## Layout

```
crates/i18n  otherwise-apps-i18n: embedded translation catalogs, t!/tn!/tv! macros, i18n guard tests
crates/core  otherwise-apps-core: catalog (+ validation), version comparison, asset selection,
             SHA-256 verification, download, unpack, install / replace / uninstall, detection,
             Linux .desktop and Windows shortcuts, running-app quit; calendar.rs (merge of the apps'
             events feeds, holidays, .ics overlays, combined otherwise.ics) and vcity.rs (device-code
             sign-in, refresh, read-only events / bookings), background.rs (the launcher's jobs:
             calendar-sync, upcoming). No UI.
crates/app   `otherwise-apps` desktop app (GPUI + gpui-component). app.rs: state, checks, jobs;
             cards.rs: grid / list; dialogs.rs: modals, settings, About; jobs.rs: background work;
             hub.rs: Today page, Entities page, Settings → Hub, otherwise:// handling;
             calendar.rs: Calendar page (⌘5: month / week / agenda, filters, sources modal,
             combined ICS, Vcity sign-in; core calendar.rs + vcity.rs hold the logic), secrets.rs
             (keychain);
             data_root.rs: Settings → Otherwise data folder (otherwise-link `relocate`: plan,
             confirmation listing what moves, verified move / copy / use-existing, cloud status,
             registry lock and conflicted-copy warnings)
             agent.rs: the AI panel (otherwise-agent-panel, docked next to every page, ⌘J): its
             AgentHost (hub as MCP server, `mcp --read-only` for Solo lectura, page context),
             Today "Preguntar"; main.rs makes the AgentDock the window's root view;
             Vcity invitation in About, Help menu and under the apps grid
             (`otherwise-apps mcp` runs the otherwise-hub MCP server, main.rs)
             helper.rs: `otherwise-apps background manifest|run <job>` (no window), starting
             Otherwise Helper, Settings → Otherwise Helper
crates/helper otherwise-helper (lib + bin, **no GPUI**): Otherwise Helper, the menu-bar process
             that runs installed apps' background jobs (otherwise-link `background`): engine.rs
             (scan, verify-before-run, state, notification drain), sched.rs (due / waits: pause,
             Low Power Mode, memory pressure, offline, app open; backoff, jitter), runner.rs
             (timeout, memory ceiling, process group), sys.rs, menu.rs (menu as data), state.rs
             (<device dir>/helper/state.json, never the data root), mac.rs (NSStatusItem, UNUserNotificationCenter,
             SMAppService). Bundled as Otherwise Apps.app/Contents/Library/LoginItems/
             Otherwise Helper.app by script/bundle-macos.sh.
crates/server otherwise-server (lib, **no GPUI, no app code**): Otherwise Helper's server mode,
             otherwise-link SPEC §12 `/v1` on 127.0.0.1 only, published with `tailscale serve`:
             identity.rs (serve headers + `tailscale whois`; loopback is NOT trusted: every
             connection's other end must be the Tailscale app's own signed process, via lsof +
             codesign), config.rs (<device dir>/server/server.json: shared docs, members, roles,
             tier, backups), supervisor.rs (the apps' `serve` processes from their manifest, idle
             stop, memory ceiling, restart on update), http.rs, seats.rs (30-day distinct members,
             counted and logged only: no telemetry), audit.rs (jsonl, argument hashes only),
             idem.rs (idempotency keys, 48 h), backup.rs (nightly snapshots, 14/8/12).
             The helper's `server run|status|doctor|init|share|unshare|member|tier`
             (helper/src/server_mode.rs) drives it; `run` also runs the jobs worker unless the
             menu-bar helper does. Tests: a fake Tailscale and a shell-script app, never the
             network.
tools/icon   resvg rasteriser used by script/build-icons.sh (own workspace)
```

## Rules

- **Never install anything unverified.** Every download is checked against GitHub's asset
  `digest` (SHA-256); no digest or a mismatch → the file is deleted and nothing is installed.
  Catalog URLs are validated (org repos, the app's own release downloads only).
- **Never delete user data or an installed app.** Replaced and uninstalled apps go to the Trash
  (`install::system_trash`); the only folders removed outright are our own temp unpack and
  download folders. App settings/data dirs are never touched. Uninstall only accepts paths from
  `Layout::candidates`.
- Running apps are asked to quit only after the user agrees.
- **i18n is mandatory.** No user-facing string literal in `app/src` or `core/src`; use
  `i18n::t!` / `tn!` and add the key to **every** `crates/i18n/locales/<lang>/` catalog in the same
  change. Untranslatable literals end their line with `// i18n-ok`. Catalog descriptions of the
  apps come from the manifest (en/es/fr/pt), not from the i18n catalogs.
- Tests never touch the real Trash, /Applications or the network (except `tests/live.rs`,
  `#[ignore]`d, which installs into a temp dir). Every core change ships with a test.
- `crates/core/data/catalog.json` is the bundled fallback: a snapshot of
  https://otherwisetech.com/apps.json. Refresh it when apps are added.
- The product is **Otherwise Hub** (window, menus, About, footer, Dock / Finder through the
  bundle's localized display name, catalog `name`, KNOWN name). Kept on purpose: the bundle file
  `Otherwise Apps.app` (catalog `appName`), `io.github.otherwisetech.otherwise-apps`, the
  `otherwise-apps` binary / install name / app id / data folder, `Otherwise Helper.app` and its
  login item: self-update, discovery, MCP paths and registry ids depend on them. Releases attach
  `otherwise-hub-<platform>.zip|.tar.gz` and the same bytes as `otherwise-apps-<platform>…`,
  which the catalog and every installed copy update from: keep both.
- **The hub uses otherwise-link** (github.com/otherwisetech/otherwise-link, by tag): discovery,
  links, Today files, the entity registry and `hub.json` follow its docs/SPEC.md. The shared
  registry stays off until the user turns it on; MCP write tools stay off until the user
  enables them. Never edit the user's Claude / Codex / MCP config: Settings shows the
  `claude mcp add` command to copy. Forward other apps' links with `open_app_link` (never back
  to `otherwise://`). Merges and deletes in the registry go through a confirmation.
  The AI panel (github.com/otherwisetech/otherwise-agent-panel, by tag) passes the hub to the
  agent CLI only on its command line / environment (temporary `--mcp-config`, `-c` overrides,
  `OPENCODE_CONFIG_CONTENT`), turns off its built-in file / shell / web tools, and never writes
  the user's agent config; its tests use fake CLIs. History stays under
  `<data root>/otherwise-apps/assistant/` (0.3 conversations load).
  Calendar: read-only over the apps' `events.json`; busy-only events never leave the page with
  details (ICS export, agent context and MCP get "Busy"); .ics URLs are fetched only on add /
  refresh and by the helper's hourly `calendar-sync` job (stale copies only; a failure keeps the
  last copy); Vcity tokens only in the OS keychain (`secrets.rs`), never logged or written to the
  data root, redirects refused; Vcity tests use a local mock server, never vcity.io.
  Moving the data root always goes through the confirmation that lists what moves, and is
  refused while an app has its data open (its lock).
- **Otherwise Helper runs nothing for apps that aren't installed.** Jobs come only from the
  `otherwise-background.json` inside an installed bundle (never the data root); every run
  re-verifies the install (`background::plan`); gone apps are dropped quietly (no error loop,
  no notification), their outbox and data left alone. The helper never downloads, installs or
  suggests installing anything, and links no app's code. It stays GPUI-free (target: tens of
  MB idle). Start at login is on by default after the first launch with the helper, off with
  one switch in Settings.
  **One computer per data root**: per app, only the holder of the otherwise-link lease
  (`<root>/<app>/background.lock.json`) runs its jobs and drains its outbox; others show
  "running on <host>". Device state (runs, backoff, pause, shown) lives in
  `paths::device_dir()`, never in the data root (0.6.0's `~/.otherwise/helper` is moved once).
- **Server mode never trusts loopback** (CI runners on the server computer run as the same
  user): no request is answered without a verified Tailscale identity, `trustLoopback` is for
  test servers only (logged as a warning), the server binds 127.0.0.1 and never changes
  Tailscale's configuration (it prints the `tailscale serve` command). Documents not shared
  with a member are 404. Seats are counted locally; nothing is sent to Vcity.
- Comments: sparse, only non-obvious why.
- GPUI pins: `Cargo.lock` pins the same zed / gpui-component revs as the other apps. Never
  `cargo update` them; building only core can prune them from the lock — restore it.

## Validate

```
cargo test -p otherwise-apps-i18n -p otherwise-apps-core -p otherwise-apps-app -p otherwise-helper
cargo clippy --workspace --all-targets
```
