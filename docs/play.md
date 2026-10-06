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
   the zone connection is made, the playfield is loaded and the loading screen dissolves into it. Esc quits in the world;
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
(x ~ 193, z ~ 157) made the server send `PlayfieldAnarchyF` 4582 (Newland City, start at 931, 20, 729) -> the loading screen -> the new world with terrain following and jumping.
Headless equivalents: `flow::tests::{own_character_walks_from_the_keyboard, autopilot_crosses_the_arrival_hall}`.

## In the world (M3 step 1)

After `ZoneLogin` the zone's N3 stream (docs/zone.md) drives the flow: `PlayfieldAnarchyFIIR_t` names the playfield to load (so a freshly created character
starts in its own start playfield, e.g. 4604 Arrival Hall, not in the list row's), the camera is placed at the player's `SimpleCharFullUpdateIIR_t`
position (server `(x, y, z)` → scene `(x, y, -z)`, eye 1.7 m) facing its heading, and `CharInPlayIIR_t` is sent once after 10 world frames. Pings are answered
by the session thread. `AOMAC_NET_TRACE=<file>` records every frame of the login and zone connections (`<ms> > | < <hex>`, credentials/cookies redacted) for protocol work;
`cargo run -p ao-net --example probe -- --login --select N --wait 90 --out FILE` does the same without the GUI (credentials from the TTY / stdin, never arguments).

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
`AOMAC_PREFS_DIR` redirects the prefs directory. Both skip without the client.

`AOMAC_SHOT_DIR=<dir> cargo test --release -p aomac shots -- --nocapture` renders login, character select, the delete dialog and every
creation scene (real mouse/keyboard input at window coordinates: breed pick, head/height/build, profession, name, close → exit dialog)
offscreen through `ao_render::Offscreen` (same frame sequence as the windowed app) into PNGs. No window or unlocked session needed.

## In-world HUD (HudLayout)
`play/hud.rs` (`Hud`, `WindowKind`) builds the default interface on entering the world: `ControlCenter.xml` overlay with wings, bottom bars, left/right menus (`ActionMenu/*.xml`, sub-menu popups, click toggles the `dvalue` and opens the window kind), health/nano/XP/alien bar windows driven by `Zone::stat`, the compass window (`hud_compass.rs`: strip scrolled by the own heading, waypoint marker), the AGG/DEF slider (`hud_aggdef.rs`: dragging sends `SetStatIIR_t` 0x33 on release via `Hud::take_outbox`) and the shortcut bar (`hud_bar.rs`: first-login Start Combat / Walk / Sit / Follow macro / Suspended Animation slots with rdb 1010008 icons, tooltips, use, drag-and-drop; macro use runs through `Chat::run_line`, special-action uses wait in `Hud::take_uses` for the movement / combat plumbing). Details, addresses and gaps: docs/gui.md §10.

## Controls (Controls)
`play/controls.rs` (`Controls`, `Cmd`, `CamCmd`, `ControlPrefs`) and `play/camera.rs` (`Camera3p`) are pure state machines for the own character's input and camera; the evidence, addresses and unresolved items are in docs/zone/camera.md.

**Default bindings** (from the client's `CharPrefs.xml` `KeyBindings`, provider = hash of the name in `FlowControlModule_t`; a test reads the installed file): W / middle mouse button forward, S back, A turn left, D turn right, Z strafe left, C strafe right, Space jump, Backspace walk/run toggle, Numpad 0 auto-run, arrow keys = forward/back/turn (work while typing only with Ctrl/Alt), F8 first/third person, Shift+F8 / Ctrl+F8 previous/next scripted camera view (not implemented), F12 screenshot, R pick up item. Fixed camera keys: Numpad 4/6/2/8 rotate, Numpad +/− zoom, Numpad 7 save camera position, Numpad 5 reset. There is no default sit key (nor crawl). Press → `Cmd::Move(1,3,5,7,9,0xC)`, release → the next action (2,4,6,8,0xB,0xE); jump 0xF and auto-run (1) act on press only.

**Mouse**: left drag orbits the camera, right drag turns the character (and pitches the camera), Ctrl + right drag orbits; a press/release with ≤ 0.02 of movement is a click (`Cmd::Click`, left = select, right = default action). 100 mouse counts at the default sensitivity 10 = 1 rad. Turn keys become strafes while a look is active. Wheel = zoom (`MouseWheel` 0), 2 m per notch at `ZoomSpeed` 20, camera distance 0.78…25 m (default 5 m behind, 18.4° above the look target), zooming in past 0.8 m switches to first person. Horizontal field of view 90°. Free-fly is not part of the original client (GM debug only), so play mode drives `Camera3p`.

**Wiring** (integration owner): per frame `host.look = controls.mouse_capture()`; keys → `controls.on_key`, mouse → `on_mouse_button/on_mouse_motion/on_wheel`; route `Cmd::Move(a)` → `Movement::action`, `MouseTurn` → `Movement::mouse_turn`, `Camera(c)` → `camera.apply(&c)` (an `EndLook` result is a heading sync for the avatar), then `camera.update_with(avatar_scene_pos, avatar_yaw, dt, &line_of_sight)` → `host.camera` and `lens` from `camera::vertical_fov(aspect)`. Chat input focus: `controls.set_text_input(true)`.
