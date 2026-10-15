# `aomac play` — the windowed client (M2)

`aomac` with no arguments, or `aomac play`, opens the original login flow (spec + RE evidence: `docs/screens.md`; GUI engine:
`ao-gui`): `LoginWindow` over the `LoginWorld_c` 3D backdrop → `ProgressDialog` → `CharacterSelectionWindow` with the
`CharacterViewer_c` preview → loading screen (`ai_loading_login.png`, 4 s fade-in, 7 s fade-out) → the character's playfield.

## Logging in

1. `cargo run --release -p aomac` (or `aomac play`; `--server Ithaca` picks a server by name — the status API lists them, the
   chosen one is printed on stderr and remembered; default: last used, else the first).
2. Type the username, then the password (focus starts in the password field as in the original; typing a username clears the
   password field, exactly like `SlotUsernameModified`). Login enables once both are non-empty. Enter or Login connects.
3. After a successful login the username (never the password) is remembered in
   `~/Library/Application Support/aomac/prefs.txt` (stand-in for `prefs/Prefs.xml` `LauncherConfig`; combo box + Remove work).
4. Pick a character (click, Up/Down; the 3D preview follows), **Play** or Enter. After `ZoneHandoff` the loading screen fades in,
   the zone connection is made, the playfield is loaded and the loading screen dissolves into it. Esc does not quit in the world (the original has no such binding: docs/chat/dialogs.md §3; closing the window or `/quit` does);
   The own character is controlled with the client's key bindings (see "Controls"); free-fly is used only when the avatar could not be built (the original has no free camera).

Errors: `ShowError(code, arg)` opens `<ERRORURL><code>[-<arg>].html` (ERRORURL from `AnarchyLauncher.url`) in the default browser
(`open`), the login window stays up (docs/screens.md §3.6). Connect failure → 1; login refused (0x0d) / rejected (0x21) → type + detail;
login connection lost → 3 ([INFERENCE]). Non-login notices (character slots exhausted, playfield load failure, in-world) use a
ProgressDialog-based box with "OK".

Clipboard: Cmd+C/X/V/A in text fields use the OS clipboard (`arboard`); the password field never copies/cuts (`tvf::PASSWORD`).

## Own character in the world (`play/player.rs`)

`Player` glues `controls.rs` (client key bindings, mouse-look), `movement.rs` (the client's movement FSM; docs/zone/movement.md), the playfield `Collision`
(ao-formats, docs/zone/collision.md), `avatar.rs` (own model on the actor layer, docs/zone/avatar.md) and `camera.rs` (third-person camera, docs/zone/camera.md).
It is built when the world appears (and again whenever the server announces the own dynel again, e.g. after a teleport) from the own
`SimpleCharFullUpdateIIR_t` (`Zone::own_update`), starts at its position/heading (snapped to the collision ground) and takes the stats from `Zone::stats`.
Every frame: keys -> `Movement::action`, `Movement::update` -> `CharDCMoveIIR_t` frames on the zone connection (exactly the messages the FSM emits: key
actions, a sync every 5 s while moving, ...), avatar pose from the FSM state, camera behind the avatar. `CharDCMove`s the server relays for the own
dynel are ignored, as in the original (`FUN_1006bcc6`). The login window's text focus is cleared when the world appears (it blocked every key).
`Frontend::game_input` (ao-render) delivers physical keys and mouse-look deltas, `Host::look` captures the cursor.

Live (Ithaca, 2026-10-06, Aomacvolk): `cargo test --release -p aomac live_walk -- --ignored --nocapture` (stdin: user, password; `AOMAC_LIVE_STEPS`, see
`play/flow/live.rs`) drives the real `Play` offscreen through `ao_render::Offscreen` (no window; needed because a locked screen never renders windows): login, character
pick, zone, then key steps and a collision-route autopilot (`goto=x:z`). Results: the server accepted every move (positions persist across relogs: the next login started
exactly where the previous run stopped); walking from the Arrival Hall start (205.2, 1.0, 255.8) around the exhibits and through the central ICC shuttleport tunnel
(x ~ 193, z ~ 157) made the server send `PlayfieldAnarchyF` 4582 (ICC Shuttleport, start at 931, 20, 729) -> (that build showed the loading screen; the original does not, the zone change now follows docs/zone/world.md §10.2: interface kept, "Changing area. Please wait.") -> the new world with terrain following and jumping.
Headless equivalents: `flow::tests::{own_character_walks_from_the_keyboard, autopilot_crosses_the_arrival_hall}`.
Harness notes (`play/flow/live.rs`): `ui=ctrl+5` / `ui=shift+p` / `ui=f10` are `press` strokes (`+`-separated key names, modifier keys update `host.mods`) through `Play::game_input`, where the window hot keys live; `say=` needs a leading `/` (`say=/say hi`, `say=/open Faction`; `run_line` drops plain lines); `goto` on a one-cell route arrives at once; `water[=radius]` prints the nearest deep-water ground points (server coordinates, 4 m lattice up to 300 m, coarser beyond) for a swimming `goto=x:z`; `AOMAC_LIVE_CC=<name>` clicks through the four creation scenes with real mouse events (shots `cc-0..3`), Next / Finish included: the character is **really created** (use `Aomac<random>`, announce it) and the session continues in the world with `AOMAC_LIVE_STEPS`; `login` waits for the login screen after `say=/camp` (30 s) and prints the window / chat / session teardown state; `AOMAC_LIVE_AVOID=<metres>` makes `goto` route around living hostile (side 3) NPCs by that distance, and a `goto` ends when the own character dies (a walk through a hostile camp ends in the respawn at the playfield start, docs/zone/movement.md §4.1) or when a zone change drops the own dynel; the route search stays in an ellipse around start and goal (an unreachable goal such as a closed door must not flood the playfield); a `charselect` shot of the picked row is taken before entering the world.
`buffs` prints the own timed nano entries, NCU cost and nano clock; use it at login and after casting alongside `spells` (active stat effects) to distinguish a persisted nano from a new cast.
A standing jump stays in place (docs/zone/collision.md, Fall-start callback: `Vehicle+0xcc` is the horizontal speed only); leaving the world for the login closes the chat / interact layers (docs/chat/dialogs.md).

## In the world (M3 step 1)

After `ZoneLogin` the zone's N3 stream (docs/zone.md) drives the flow: `PlayfieldAnarchyFIIR_t` names the playfield to load (so a freshly created character
starts in its own start playfield, e.g. 4604 Arrival Hall, not in the list row's), the camera is placed at the player's `SimpleCharFullUpdateIIR_t`
position (server `(x, y, z)` → scene `(x, y, -z)`, eye 1.7 m) facing its heading, and `CharInPlayIIR_t` is sent once after 10 world frames. Pings are answered
by the session thread. `AOMAC_NET_TRACE=<file>` records every frame of the login and zone connections (`<ms> > | < <hex>`, authentication usernames, responses and cookies redacted) for protocol work;
`cargo run -p ao-net --example probe -- --login --select N --wait 90 --out FILE` does the same without the GUI (credentials from the TTY / stdin, never arguments).
`Conn` masks the username in both `UserLogin` and `UserCredentials`, plus the credential response, before any wire tap receives the frame; frame lengths and non-secret fields remain unchanged. Regression: `ao-net` test `authentication_tap_redacts_usernames_and_response_without_changing_wire` uses synthetic authentication data and checks that the server still receives the original bytes.

## App bundle

`scripts/bundle.sh` → `target/aomac.app` (release build; `Contents/MacOS/aomac` execs `aomac-bin play`; Info.plist: high-res capable,
games category, macOS ≥ 12). The client is found from `$HOME/Games/ProjectRubiKa/client` (HOME is set when launched from Finder;
`bundle.sh` honours `AOMAC_CLIENT` only for the icon). The icon is the client's own: `AnarchyOnline.exe` RT_GROUP_ICON (identical to
`Anarchy.exe`'s), extracted by `scripts/exe_icon.py` at bundle time (not committed). The exe carries only a 32×32 4-bpp image, so larger
`.icns` sizes are upscaled. Verified: `open target/aomac.app` shows the login window; Cmd+V/A/C round-trip through the clipboard.

## Deliberate deviations / not implemented (all labelled)

* **Server choice** is outside the original window (the original gets the server from the PRK launcher's `IA`/`IP` arguments):
  `--server` + remembered, no in-window picker.
* New Character opens the original character creation, Delete the name-confirmation window (docs/screens.md §12); live creation was exercised against Ithaca (docs/protocol.md §8: random name, NameInUse, created, list update, start in the Arrival Hall). The creation cameras use the full `CharCreateCamera.dat` orientation incl. roll (`ao_render::Camera::roll`); the name-scene error sounds `SM_Sandy_CC_Name` / `SM_Sandy_CC_Nick` exist in no sound bank, so (like the original) they are silent; profession hover/leave/click text rules: docs/screens.md §12.
* Row "Inactive" uses bit 0 of the list's per-row `status` word as RE'd (`docs/screens.md` §5.3); PRK sends `status = 1` for ordinary characters
  (live, both rows of the test account) and the rows are shown as normal (not "Inactive").
* Preview socials are built on demand in the background (idle plays until ready); the original has no delay.
* Preview lighting/FOV convention: UNRESOLVED (renderer default; `docs/screens.md` §11).
* The startup music cue plays at the zone hand-off; the original starts it on `CharacterLoggedInMessage` (0x26), a local AFCM message after the zone TCP connect
  (docs/protocol.md §8 "Zone hand-off timing"), which is within a second of our hand-off + 4 s delay.
  (state is tracked in `play/zone.rs`, plan in docs/zone.md §6).

## Debug flags (hidden)

`--fake-charlist [--select N]` shows a built-in list (offline character-select check; Play then runs only the loading screen).
`AOMAC_PERF=1` prints preview build timings. With `--fake-charlist` an in-process fake server answers New Character (random name "Zalokon", "Taken" → name in use, else created + hand-off to the loading screen) and Delete; `AOMAC_CC_SKIP_INTRO=1` jumps to the breed scene; Esc in creation skips the running camera move (docs/screens.md §12).

## Headless tests

`crates/aomac/src/play/flow/tests.rs` drives the real `Play` state machine (real client GUI XML/skin, no window) through
Login → Cancel / timeout → late `Bg::Connected(Ok/Err)` (stale results are dropped, the session closed, the hidden LoginWindow is
shown again, not recreated), and the `SlotLoginReply` routing: `LoginError` → `ShowError(0x0d, code)`, `Rejected` → `(0x21, detail)`
(signed `%d`, also during character creation), connection lost → `(3,0)`. Error URLs are recorded instead of opened under `cfg(test)`.
`AOMAC_PREFS_DIR` redirects the prefs directory and must be absolute. Empty overrides are treated as unset and use an absolute `HOME` plus `Library/Application Support/aomac`; missing/empty or relative bases emit an error and disable persistence rather than writing into cwd. The live harness uses its thread-private scratch directory when the override is unset or empty, and rejects invalid explicit overrides. Both skip without the client.
Tests use `play::prefs::test_dir()` for a stable, thread-private scratch directory, unique across test processes and removed when the test thread exits; `set_test_dir` keeps the preference override thread-local. Social/LFT save-and-reopen tests must never share or clear another test's directory.

`AOMAC_SHOT_DIR=<dir> cargo test --release -p aomac shots -- --nocapture` renders login, character select, the delete dialog and every
creation scene (real mouse/keyboard input at window coordinates: breed pick, head/height/build, profession, name, close → exit dialog)
offscreen through `ao_render::Offscreen` (same frame sequence as the windowed app) into PNGs. No window or unlocked session needed.

## In-world HUD (HudLayout)
`play/hud.rs` (`Hud`, `WindowKind`) builds the default interface on entering the world: `ControlCenter.xml` overlay with wings, bottom bars, left/right menus (`ActionMenu/*.xml`, sub-menu popups, click toggles the `dvalue` and opens the window kind), health/nano/XP/alien bar windows driven by `Zone::stat`, the compass window (`hud_compass.rs`: strip scrolled by the own heading, waypoint marker), the AGG/DEF slider (`hud_aggdef.rs`: dragging sends `SetStatIIR_t` 0x33 on release via `Hud::take_outbox`) and the shortcut bar (`hud_bar.rs`: first-login Start Combat / Walk / Sit / Follow macro / Suspended Animation slots with rdb 1010008 icons, tooltips, use, drag-and-drop; macro use runs through `Chat::run_line`, special-action uses wait in `Hud::take_uses` for the movement / combat plumbing). Details, addresses and gaps: docs/gui.md §10.

Item-use integration: `Interact::take_outbox` records each drained interaction request in the use-action send map; the HUD flush in `Play::frame` records each HUD request once before sending it. `Hud::activate_item` immediately drains `HudStats::take_outbox` into the HUD outbox, so hotbar activation does not wait for another inventory event. Incoming interaction frames and `use_action_frame` pass mutable `Zone` state to the use-action runtime; mission, loot, grid and trade replay fixtures use the same mutable entry point. Source: `play/interact.rs`, `play/flow.rs`, `play/hud.rs`, `play/interact_play.rs`, `play/combat/glue.rs`. Regression: `hud::tests::hotbar_item_activation_flushes_without_a_mouse_event`.

### Live HUD acceptance (Ithaca, 2026-10-06, Aomacvolk, offscreen)
The desktop session was locked (`CGSSessionScreenIsLocked`), so no window can render or take osascript keys: the acceptance ran through the offscreen live harness (`play/flow/live.rs`, same `Play` / `Offscreen` frame path as the window, 1280×800, credentials on stdin) with the HUD steps `ui=<hotkey>`, `move`, `click`, `clickdyn=<id>`, `mdrag`, `watch`. Aomacvolk logged in at server (931, 20.6, 729) facing south; the zone sent 28 dynels within 60 m.
* **Default layout** (identical to the headless `hud-1280.png` apart from live data): left wing with 5 round menu buttons + the `NCU 0/8  CRED 1,000` bar, right wing menu, compass (centre at 0.73·W, heading S then W/N as the avatar turned), shortcut bar with Start Combat / Walk / Sit / Follow (yellow label) / EXIT slots, vertical health + nano bars (red / blue full), XP bar on the right edge, AGG/DEF slider at the far right (knob at AGG = stat 0x33 = 100 as the server sent it), chat windows bottom centre with the Ithaca welcome lines, NewChar-template windows Wear (top right) and Stat (right, name `Aomacvolk`, level 1, health 34 / 1, nano 32 / 1, XP rows 0/1450 0/1500 0/2000, ACs) overlapping the right menu and the bars at this small screen size (template frames are clamped, not re-laid-out).
* **Hotkeys**: `U` Skills (full window with IP 1500, skill groups), `I` Inventory (3-column grid at the bottom right, clamped inside the screen, scrollbar), `Ctrl+1` / `Ctrl+9` toggle Wear / Stat (closed on the first press: they are open from the template), `Ctrl+6` PF Map (tab "PF Map", terrain raster, own marker), `P` Planet Map (zoom buttons, tiles; the character's playfield is not on the planet, so the view shows the black unmapped area with the map in the lower right).
* **Menu buttons**: hover over a left button fades in its label ("Nano") and after 500 ms the tooltip ("Programs Window" / body text); a click on "Actions" opens the sub-menu popup (Emotes … Help/Settings) above the button; a click on "Inv" opens the inventory, a click on "Wear" closed the open Wear window.
* **World click**: `clickdyn` hit the Surf Lizard (10 m away) → target `1037254`, the selection bracket + name tag + yellow health line over the model, and the top target tube with the "Selection / Surf Lizard" window, as in the headless `target-hostile.png`. The attack on a second target was refused by the server ("PvP not allowed in this district"), so no damage was taken.
* **Hotbar tooltip**: "Start Combat" appeared after the pointer rested on slot 0.
* **Live stats**: dragging the AGG/DEF knob to the middle set stat 0x33 to −5 (sent as `SetStatIIR_t`); clicking the Sit slot made the server send `StatIIR_t` (stat 425 None→25, stat 430 0→2) and the avatar sat down, a second click stood it up again (425 → 100, 430 → 0).
* **Bug found and fixed**: the alien XP bar (`CCAlienXPBarConfig` has no frame in the NewChar template) was placed at the screen corner (0, 0); it now sits 10 px beside the XP bar (left of it when that leaves the screen; **unresolved guess**, test `hud::tests::alien_bar_sits_beside_the_xp_bar`).
* **Remaining gaps**: health / nano show `34 / 1`, `32 / 1` because PRK's FullCharacter stats carry Life (1) = 1 and MaxNanoEnergy (221) = 1 while the dynel header says 34; the original reads `N3Msg_GetSkill(1, 2)` (no trickle-down entry for stat 1 exists in rdb 1000204, checked) so the same text is expected unless the server sends a later `StatIIR`, unverified against retail. The programs (Nano) window and other kinds without a `play/hud_*.rs` window do nothing on click. Yellow floating numbers (DamageText) were drawn stacked near the top of the screen at x ≈ 675 without a fight of our own; position source not found (combat glue, not HUD). A real window / osascript run is still outstanding until the screen is unlocked.

## Controls (Controls)
`play/controls.rs` (`Controls`, `Cmd`, `CamCmd`, `ControlPrefs`) and `play/camera.rs` (`Camera3p`) are pure state machines for the own character's input and camera; the evidence, addresses and unresolved items are in docs/zone/camera.md.

**Default bindings** (from the client's `CharPrefs.xml` `KeyBindings`, provider = hash of the name in `FlowControlModule_t`; a test reads the installed file): W / middle mouse button forward, S back, A turn left, D turn right, Z strafe left, C strafe right, Space jump, Backspace walk/run toggle, Numpad 0 auto-run, arrow keys = forward/back/turn (work while typing only with Ctrl/Alt), F8 first/third person, Shift+F8 / Ctrl+F8 previous/next scripted camera view (not implemented), F12 screenshot, R pick up item. Fixed camera keys: Numpad 4/6/2/8 rotate, Numpad +/− zoom, Numpad 7 save camera position, Numpad 5 reset. There is no default sit key (nor crawl). Press → `Cmd::Move(1,3,5,7,9,0xC)`, release → the next action (2,4,6,8,0xB,0xE); jump 0xF and auto-run (1) act on press only.

**Mouse**: left drag orbits the camera, right drag turns the character (and pitches the camera), Ctrl + right drag orbits; a press/release with ≤ 0.02 of movement is a click (`Cmd::Click`, left = select, right = default action). 100 mouse counts at the default sensitivity 10 = 1 rad. Turn keys become strafes while a look is active. Wheel = zoom (`MouseWheel` 0), 2 m per notch at `ZoomSpeed` 20, camera distance 0.78…25 m (default 5 m behind, 18.4° above the look target), zooming in past 0.8 m switches to first person. Horizontal field of view 90°. Free-fly is not part of the original client (GM debug only), so play mode drives `Camera3p`.

**F12 screenshot** (retail `GUI.dll` `FlowControlModule_t::SlotScreenshot`, which makes an unconditional virtual call into `DisplaySystem.dll`'s backbuffer capture routine): PNG, zlib level 9, folder `AnarchyPath_t::GetScreenshotsPath` = `%LOCALAPPDATA%\Funcom\Anarchy Online\Screenshots\` (created if missing), file name `scrn%04d.png` against a process-lifetime counter that keeps counting up past any name already on disk, so it never overwrites. No chat feedback line exists anywhere in retail's GUI/Gamecode/Interfaces/DisplaySystem modules for this command (`DumpRefs` for "Screenshot"/"ScreenShot" across all four only hits the `#CameraScreenshot` command name and the `SlotScreenshot` symbol itself), so aomac stays silent too. aomac's equivalent folder is the prefs app-support dir's `Screenshots` subfolder (`play::prefs::next_screenshot_path`), never the client install dir. The capture itself re-renders the just-drawn frame (world + GUI) into a one-off render-to-texture (not `COPY_SRC` on the swapchain, which macOS `CAMetalLayer` drawables don't reliably support) and PNG-encodes it on a background thread so the slow zlib compression never stalls the render loop (`ao_render::capture_frame_to_png`).

**Wiring** (integration owner): per frame `host.look = controls.mouse_capture()`; keys → `controls.on_key`, mouse → `on_mouse_button/on_mouse_motion/on_wheel`; route `Cmd::Move(a)` → `Movement::action`, `MouseTurn` → `Movement::mouse_turn`, `Camera(c)` → `camera.apply(&c)` (an `EndLook` result is a heading sync for the avatar), then `camera.update_with(avatar_scene_pos, avatar_yaw, dt, &line_of_sight)` → `host.camera` and `lens` from `camera::lens`. Chat input focus: `controls.set_text_input(true)`.

## Headless performance comparison

`AOMAC_PERF=1` makes `Offscreen::frame` render the real 3D scene and GUI every tick to one
persistent attachment, then wait for that frame's GPU submission. Without it, the harness
remains logic-only except explicit screenshots. No readback/PNG encoding occurs in timed
frames. Reports include CPU **wall** durations and calling-thread CPU time from
`clock_gettime(CLOCK_THREAD_CPUTIME_ID)` (Darwin 16 / Linux 3). Thread CPU excludes
descheduling, GPU time, and background asset/driver workers; it is not process-wide CPU.
No GPU timestamp queries are used. Stage boundaries are:
`update` = renderer sky advance/visibility, `frontend` = `Play::frame` (including simulation,
asset completion and GUI layout), `upload` = host policies/scene/actor application,
`render`/`gui` = command preparation and submission, `gpu_wait` = final submit/completion.
`total` includes all those stages, excludes harness sleeps, reporting, and screenshots.
Reports give mean and nearest-rank p50/p95, bounded to 600 samples per interval.
RSS comes from `ps` in KiB at report time. Metal `currentAllocatedSize` is native device
allocation in unified memory, **not dedicated VRAM**. Tracked creations/estimated created
bytes are cumulative, not retained allocation; `AOMAC_GPU_STATS=1` optionally adds call-site
creation logs. Unsupported native allocation/RSS observations are explicitly unavailable.

For baseline, apply **only the instrumentation patch** to `origin/main`; for after, apply
the optimizations to that same instrumented revision. In each checkout run exactly:

```sh
AOMAC_PERF=1 AOMAC_AUDIO_MUTE=1 CARGO_TARGET_DIR=/tmp/PerfRound2/wt/target \
  cargo test --release -j 2 -p aomac offscreen_icc_performance_replay -- --ignored --nocapture --test-threads=1
```

This existing flow replay route consumes `docs/captures/zone_ithaca.rec` (Testy, id 25988),
waits for InWorld and actor readiness, reports handoff-to-ready `zone_load_ms`, fixes the
ICC camera to capture spawn + `(0, 4.6, 12)` looking at spawn + `(0, 1.6, 0)` in renderer
coordinates (server Z negated), warms 120 frames, then measures 600 frames at fixed 1/60 s.
Camera coordinates are logged. Actor animation/background sky completion can still vary;
do not treat this replay as a bitwise deterministic visual fixture. Optional
`AOMAC_PERF_SHOT=/absolute/path.png` writes just the final frame **after** measurement.

Live comparison uses the same offscreen `live_walk` harness, never a window. Acquire
`scripts/live-lock.sh acquire PerfRound2` first; release it immediately afterward.
Use the same approved test character in ICC, position, preferences and explicit renderer
camera on both revisions. Credentials are two stdin lines only (not files, argv or logs).
The camera numbers below are illustrative, not an observed ICC location. First log
`cam` in the baseline session, then reuse those actual coordinates for both revisions:

```sh
AOMAC_PERF=1 AOMAC_AUDIO_MUTE=1 AOMAC_LIVE_CHAR=Aomacfixr \
  AOMAC_LIVE_STEPS='wait=5,perfcam=205.2:5.6:-243.8:0:-0.244979,perf=600' \
  CARGO_TARGET_DIR=/tmp/PerfRound2/wt/target \
  cargo test --release -j 2 -p aomac live_walk -- --ignored --nocapture --test-threads=1
```

`perfcam=x:y:z:yaw:pitch` pins only the camera, not the avatar or server state.
`perf=N` resets samples and measures N fixed-60Hz frames; it excludes tick pacing.
`perfreset` / `perfreport=label` delimit other intervals. Live `zone_load_ms` spans
selection/create request through InWorld (includes network/loading fade), unlike the
replay's actor-ready endpoint; compare like with like, not the two values against each
other. Keep `AOMAC_LIVE_SHOTS` unset during timing. No live or baseline execution is
implied by these commands.

Focused CPU-only metrics check:
`cargo test --release -j 2 -p ao-render offscreen_perf_percentiles_use_nearest_rank`.

Repeat the same baseline/after replay command with `AOMAC_PERF_MSAA=1` (retail default)
and `AOMAC_PERF_MSAA=4` (enhanced), keeping all other inputs fixed. With `AOMAC_PERF`
enabled this override wins over loaded preferences in both replay and live harnesses.
Reports include requested and adapter-clamped actual sample counts; only compare matched
actual counts. Without this override preferences remain authoritative; without
`AOMAC_PERF`, `AOMAC_PERF_MSAA` has no effect. The equivalent live preference step is
`option=AomacEdgeSmoothing:0` for 1x or `option=AomacEdgeSmoothing:2` for 4x.

### Recorded single-pass comparison (2026-10-16)

Baseline: `e6c45f9` plus the measurement harness above. Each cell is the median of
three independent 600-frame ICC capture replays after 120 warmup frames, 1280×800,
fixed camera `(927.02765, 28.060379, -730.7181)`, yaw `0`, pitch `-0.24497865`.
The Mac was shared with other work; timings are observations, not isolated causal
estimates. Calling-thread CPU excludes driver/asset worker threads. Zone loading
used the warm OS file cache. Metal allocation is unified-memory device allocation,
not dedicated VRAM or a claim about physical residency.

| Configuration | Frame p50 / p95 (ms) | Thread CPU mean (ms) | Metal allocated (MiB) | RSS at report (MiB) | Zone ready (s) |
|---|---:|---:|---:|---:|---:|
| Baseline, 1× | 6.0229 / 8.2919 | 3.3707 | 456.17 | 725.36 | 6.359 |
| Single pass, 1× | 3.8047 / 5.1687 | 2.2276 | 452.05 | 690.89 | 6.373 |
| Baseline, 4× | 5.6834 / 6.7328 | 2.7963 | 484.34 | 712.67 | 6.427 |
| Single pass, 4× | 3.8837 / 5.2535 | 2.3178 | 452.63 | 697.22 | 6.392 |

Calling-thread CPU means in milliseconds, same three-run median:

| Stage | Baseline 1× | Single pass 1× | Baseline 4× | Single pass 4× |
|---|---:|---:|---:|---:|
| Update | 0.0676 | 0.0430 | 0.0549 | 0.0462 |
| Frontend | 1.6694 | 1.1743 | 1.3799 | 1.1865 |
| Upload | 0.2597 | 0.1683 | 0.2160 | 0.1785 |
| Render encoding/submission | 1.0167 | 0.6336 | 0.8827 | 0.6903 |
| GUI encoding/submission | 0.1277 | 0.0872 | 0.1089 | 0.0889 |
| Completion wait CPU | 0.2296 | 0.1235 | 0.1824 | 0.1319 |

All replays reported 347 world instances and 675 object draw calls. The new ordered
clear regression compares exact RGBA against the former split-pass attachment
clear at every supported MSAA count. Full renderer tests passed: 88 tests, 6 ignored.
Separate 640×400 frozen-time CLI captures before/after had zero differing RGBA bytes:
ICC4582 SHA256 `184894e60b2da3a82c4f645f8781556ee6195ba32206c1f51d35726c7bc7bf5b`;
Borealis800 SHA256 `91aa2aeeedb62988e4c65f691b67246ed31327c21cc8b15dbbbdacd688bbbeb0`.
These cover world/sky/liquid composition, not a claim that every sky/weather
combination was exhaustively sampled. A separate `sample` profile was taken after
the replay camera-ready marker, outside the runs used for these numbers.

Live measurements are deferred: PRK maintenance, with login explicitly prohibited
until Main announces recovery. No window, desktop input or desktop capture was used.

### Recorded shared-resource comparison (2026-10-16)

Items 11/12 were measured together, but are independent commits: native texture
sharing and `Arc<Scene>` handoff. The exact baseline is `6e8269c`; both sides were
built in the same worktree/target directory. Same camera, warmup, dimensions and
three-run medians as above. This shared-Mac observation demonstrates a small device
allocation reduction, **not a measured frame-time or zone-load speedup**:

| Configuration | Frame p50 / p95 (ms) | Thread CPU mean (ms) | Metal allocated (MiB) | RSS at report (MiB) | Zone ready (s) |
|---|---:|---:|---:|---:|---:|
| Before resources, 1× | 4.7399 / 5.3364 | 2.3079 | 452.05 | 713.12 | 5.587 |
| Shared resources, 1× | 5.4520 / 6.8850 | 2.6470 | 451.22 | 694.28 | 6.406 |
| Before resources, 4× | 4.8552 / 6.3800 | 2.3711 | 452.59 | 707.06 | 5.712 |
| Shared resources, 4× | 5.5302 / 7.8823 | 2.8283 | 451.73 | 697.47 | 6.512 |

| Calling-thread CPU stage mean (ms) | Before 1× | After 1× | Before 4× | After 4× |
|---|---:|---:|---:|---:|
| Update | 0.0431 | 0.0497 | 0.0453 | 0.0542 |
| Frontend | 1.2061 | 1.3256 | 1.2170 | 1.4030 |
| Upload | 0.1758 | 0.2063 | 0.1846 | 0.2234 |
| Render encoding/submission | 0.6568 | 0.7864 | 0.6930 | 0.8484 |
| GUI encoding/submission | 0.0889 | 0.1122 | 0.0954 | 0.1235 |
| Completion wait CPU | 0.1357 | 0.1605 | 0.1435 | 0.1759 |

After replay: 139 unique retained actor textures, 15,887,292 encoded mip bytes,
180 cumulative actor texture uploads (includes generated images and placeholders).
The deterministic two-model regression proves one native texture upload is shared,
exact rendered RGBA is unchanged, generated images never alias, differing decoded
texture families/policies do not alias, mutable mesh updates remain isolated, and
the last model releases the allocation. The Arc regression proves the upload queue
retains the original scene allocation rather than copying decoded geometry/RGBA.
These structural results are independent of noisy end-to-end timings.

## Social/economy coverage inventory (2026-10-15)

This sweep is headless only: no application windows, desktop input or screenshots. Live verification is blocked by the user's confirmed PRK maintenance; no login attempts are permitted until explicitly resumed. Existing captures remain evidence only for the particular exchanges they contain, not for new live acceptance.

| Flow | Initial inventory / evidence | Verification boundary |
|---|---|---|
| Player trade | Native `Trade_c` GC `100674ab`, GUI `100e092f`; offer/remove/abort/complete originally left bag/cash state incomplete | RE-derived synthetic two-character messages; PRK partner-template delivery remains unobserved |
| Shop selling | Existing cash-shop sold tray and captured purchases; sale inventory lifecycle needed the same trade-state correction | Synthetic sale/remove/abort/complete, not a new live sale |
| Bank/backpacks | Bank/reclaim codecs and owned-backpack archive regression existed; deposits, container transfers and frame-order routing needed coverage | Synthetic bank/backpack windows and server confirmations; no captured live bank/backpack session |
| Tradeskills | Original `TradeskillPanel.xml`, analyzer request/reply codecs existed; native sizing and container-source drops needed coverage | Synthetic analyzer/result/UI checks; compatible live ingredients and server Build confirmation not observed |
| Teams | Member/invite/leader codecs existed; `/team` membership flags were hardcoded false and first-member leadership fabricated | Replicated membership/leader and UI-action regressions; controlled second-character acceptance unavailable |
| Organisation | Outgoing `/org`, leave/disband dialogs and org-chat selection existed; incoming org server information/confirmation handling needed coverage | Native signal/packet-derived fixtures; no org membership or live org channel exchange |
| Duel/reply | Existing duel validation/wire matched native; unresolved tell names could erase the reply target | Native command/wire and synthetic reply-state regression, not a live duel |
| Friends/LFT | Existing `chat_social_ithaca.rec` covers buddy add/remove and empty search; LFT Invite, ignore refresh and Team Info actions lacked consumers | Existing capture replay plus synthetic action/UI regressions; nonempty live LFT and second-character invitations unobserved |
| Split/stack/loot rights | Split/join codecs existed without complete UI/application; own-kill loot captures do not establish unauthorised/NoDrop/team outcomes | Native qualifier/count selection and action fixtures; rights evidence is documented separately in `docs/zone/interact.md` |

Same-account concurrent-character support was checked against the public PRK issue tracker, but not established: [issue 1848](https://git.project-rk.com/prk/issues/issues/1848) concerns a stale chat connection after a disconnected game session, not two concurrent characters. The documented LoginError mapping in `docs/protocol.md` is client/CellAO evidence, not proof of PRK policy. No second live process was started to infer a policy result; player trade uses the requested native-RE/synthetic fallback.

Executed headless verification: `RUST_TEST_THREADS=2 cargo test --release --workspace -j 2` passed **1978**, with **62 ignored**, and workspace/all-targets strict clippy passed. These full gates ran **before** the subsequent additive upstream rebase (Reload and organisation decoder branches, plus appended documentation sections). After resolving that rebase, focused `ao-net` tests passed **229** and `aomac` tests passed **1181**, with **53 ignored**. This includes social/economy synthetic and recorded-capture regressions; it is not new PRK acceptance or a retail-window visual comparison.
