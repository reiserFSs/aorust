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
`winopen=<dvalue>[:off]` constructs/closes recognized HUD windows through `WindowKind::from_dvalue` and `Hud::open`/`close_kind`, matching `Hud::set_menu_value`; other controls retain the DValue-only path (e.g. social windows synchronized by their owner). `nanopage=<school index>` advances a frame after selecting the school so its program rows are rebuilt before an immediately following `nanoclick` or `dragnano`. Regression: `live_window_dvalue_constructs_and_closes_windows` checks construction, teardown and the external-owner fallback without a server session.
`dragitem=<source slot hex>:<destination slot hex>` resolves both endpoints with `Hud::item_screen_pos` at dispatch time, then uses the same ten-step GUI mouse sequence as `mdrag=x1:y1:x2:y2`; both items must be visible. Held modifiers are preserved: `down=CTRL,dragitem=40:40,up=CTRL` starts native count selection without moving between cells. No inventory commands are injected. Regression: `play_ctrl_split_mutates_locally_without_echo_and_join_waits_for_ack` covers slot resolution after a split places the new stack on a non-default visual row.
The walking `goto` pilot advances to its reached three-cell lookahead when both heading-relative offsets are inside the existing 0.2 m key deadzone, then aims ahead again; this prevents an idle hairpin corner when the next nearest-waypoint comparison cannot advance. It never searches globally for another waypoint, and swimming keeps its existing steering. Deterministic regression: `reached_walking_lookahead_advances_without_a_nearest_waypoint_jump` covers a synthetic hairpin, ordinary progression and arrival (not a reconstruction of the live route).
The live pf4582 stall at `[921.12256, 22.818768, 751.18134]` yaw 2 with `goto=920.28:756.96` came from the initial input gate: the goto started while `Play::game_input_open` was still closed (`awaiting_alive`, before the own `CharInPlay` Alive echo), so `Play::game_input` dropped the pilot's key presses, but `Pilot::set` had already recorded them as held and never pressed them again once the gate opened. `Pilot::set` now records wanted presses only while the gate is open, so `held` stays empty until then and the first step after Alive sends them. Regression: `autopilot_escapes_the_icc_ramp_stalled_poses` places the recorded own character at that start and at the earlier rounded logged pose `[920.28, 23.02, 756.96]` yaw -2.21 to `920:743` through the real `Player`/`Movement`, routes over the actual pf4582 collision, steps the pilot three times with the gate closed (held stays empty, no movement or turn), then reopens the gate and requires arrival. Separately, `installed_icc_prior_route_advances_past_the_stalled_lookahead` reconstructs the original 488-cell route and checks cell 456 against position samples within the earlier log's two-decimal rounding bin; reached lookaheads must advance and none may yield an idle decision.
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
  **Floating-number trace correction (2026-10-16):** that historical x≈675 observation is not native DamageText HUD positioning (GUI1004ae2f uses x50±20 minus integer half-width, y−20). Own third-person now follows the native world Font12122 path; only first-person uses the offscreen zero-anchor HUD (combat-log.md §6).

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

Remote ground alignment is memoized by exact position/body/surface-state bits and
the loaded collision world's generation; collision replacement and actual door
changes invalidate it. Effect connectors share bone matrices for one exact
ordered animation-layer/model generation. Head-only requests cache the original
ancestor-chain result without eagerly sampling a culled character's full skeleton.
Displayed palettes keep their existing distance cadence; translation, heading and
scale still update the world transform independently.
Behavior regressions compare exact anchor/motion bits against uncached evaluation;
`native_anchor_generation_cache_is_pixel_identical_to_recursive_connectors` renders
installed-asset connector markers and compares exact RGBA.

Effect actor IDs increase from `0xe0000000`; visual-priority history is retained across
visibility culls, but explicitly retired on effect destruction (expiry, cancellation,
source deletion, or clear). Retirement covers the original allocation: one ID normally,
32 for MeshTest, particle count for MParticle, mesh capacity for mesh effects, and scene
count for tracer meshes. Child controls own separate allocations. The host forwards
destroyed ranges without removing shared models; the renderer discards both committed
and pending native visual states at the next actor-frame application. This retains
native priority through repeated F12/capture renders of the final frame, while
holding at most one frame of retired visual history.
Regression checks: `destroyed_visual_churn_is_bounded_without_resetting_culled_priorities_or_pixels`,
`visual_retirement_covers_expiry_delete_clear_and_preserved_handles`, and
`buff_refresh_retires_gpu_models_and_same_key_appearance_stops_surface`.

Actual character/prop/corpse destruction uses an immediate reset before fresh actor
frames are applied; same-ID respawns therefore begin at authored priority without
resetting that replacement again on the next frame. Distance/visibility culls and
same-identity updates never erase native visual history. Pending resets record only
previously emitted visuals, bounding message-only spawn/despawn churn.

The existing 500,000,000-byte CPU model-cache budget is enforced within a zone,
not just at zone changes. Live character/current-next models, props, loading models
and pending clips are pinned. Unreferenced evictions also retire GPU models and
cancel queued uploads/updates so they cannot resurrect; active working sets may
exceed the budget until references disappear. Maintenance runs on actual cache or
reference changes, not a per-frame pin-set rebuild.

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

For a longer memory investigation, add
`AOMAC_ALLOC_COUNT=1 AOMAC_PERF_FRAMES=30000 AOMAC_PERF_REALTIME=1` to the replay
invocation. This runs 500 seconds of fixed-60-Hz simulation paced against a
monotonic wall clock, with memory snapshots every 600 frames and at the end.
Without these options the unpaced 600-frame workload is unchanged; its final
report label is now `replay@600` (formerly `replay`).
`AOMAC_ALLOC_COUNT` enables a **test-only** process-wide Rust/System allocator
counter at test entry; it reports outstanding allocation/count deltas and total
allocation/reallocation requests. Allocations made before enable are omitted,
but their later frees count, so signed outstanding values may be negative.
Compare snapshot deltas, not absolute heap sizes; concurrent snapshots are
approximate. Native Metal/Objective-C allocations are excluded. Allocation
counting changes overhead, so use separate uncounted runs for timing comparisons.
`AOMAC_PERF` memory reports include actor-model, actor, pooled-buffer and visual
history counts; replay/live explicit reports also include CPU dynel-cache sizes.
Zero-sample reports still emit memory snapshots, without timing percentiles.

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

### Recorded GPU-skin comparison (2026-10-16)

The measured baseline is `6e8269c` plus the texture-sharing/Arc changes, before the
GPU-skin changes and final upstream rebases (local measured snapshot `2c858d7`).
Those resource changes are published as `d2d8ebd` and `5e87597`; unrelated later
native-fidelity changes are not attributed to this performance comparison.
Same three independent ICC replays and medians, 600 completed measured frames after
120 warmup frames. These shared-Mac numbers are observations, not isolated causal
estimates. Extra immutable skin attributes/palettes increase Metal allocation:

| Configuration | Frame p50 / p95 (ms) | Thread CPU mean (ms) | Metal allocated (MiB) | RSS at report (MiB) | Zone ready (s) |
|---|---:|---:|---:|---:|---:|
| CPU skin, 1× | 5.4520 / 6.8850 | 2.6470 | 451.22 | 694.28 | 6.406 |
| GPU skin, 1× | 3.6072 / 4.9442 | 2.0659 | 452.25 | 693.64 | 6.320 |
| CPU skin, 4× | 5.5302 / 7.8823 | 2.8283 | 451.73 | 697.47 | 6.512 |
| GPU skin, 4× | 3.6327 / 5.3428 | 2.1548 | 452.80 | 696.36 | 6.412 |

| Calling-thread CPU stage mean (ms) | CPU 1× | GPU 1× | CPU 4× | GPU 4× |
|---|---:|---:|---:|---:|
| Update | 0.0497 | 0.0407 | 0.0542 | 0.0415 |
| Frontend | 1.3256 | 1.0999 | 1.4030 | 1.1244 |
| Upload | 0.2063 | 0.1095 | 0.2234 | 0.1216 |
| Render encoding/submission | 0.7864 | 0.6385 | 0.8484 | 0.6729 |
| GUI encoding/submission | 0.1122 | 0.0844 | 0.1235 | 0.0910 |
| Completion wait CPU | 0.1605 | 0.0998 | 0.1759 | 0.1035 |

The tighter fixture `native_skin_sixty_actor_performance` alternates CPU/GPU order
in one process: eight installed rigs, 60 actors, 1280×800, 32 warmup pairs discarded,
120 measured pairs per run, three runs. Both paths sample the same layers/times,
draw actual models/attachments/effect materials, and wait for completed GPU work.
Targets persist and no PNG/readback occurs in timed frames. These stages are wall
times, **not** calling-thread CPU or GPU timestamps:

| Paired crowd stage, p50 / p95 (ms) | CPU skin | GPU skin |
|---|---:|---:|
| Pose preparation | 0.3293 / 0.3916 | 0.1020 / 0.1213 |
| Actor upload | 0.5673 / 0.6946 | 0.1771 / 0.2241 |
| Render encoding/submission | 0.3640 / 0.4190 | 0.4134 / 0.4968 |
| GPU completion wait | 1.7373 / 1.8297 | 1.5688 / 1.6772 |
| Completed frame | 3.0173 / 3.2284 | 2.2583 / 2.5449 |

This fixture observed roughly 25% lower median completed-frame latency, while
compute encoding itself costs more CPU wall time. Exact vertex-byte and rendered
RGBA comparisons passed for the same eight rigs individually and in 60-actor
crowds across bind/animated transitions, including heads, weapons, Light Bar and
native material effects. Separate own/remote Deformer regressions and 35 avatar
tests passed: source-mesh effects still receive CPU geometry, and ordinary
submissions stay palette-only. Kernel safety tests cover active bone bounds,
attribute-count mismatch, duplicate IDs and pending initialization across two
submissions before rendering. A separate warmed `sample` profile observed palette
sampling rather than the ordinary CPU vertex-skin loop; sampling was excluded
from timings. Live measurements remain deferred by the maintenance restriction.

Final integration after native-filtering publication passed
`cargo test --release --workspace -j 2`: 2091 passed, 64 ignored, plus strict
workspace/all-target release clippy. The own-body selection regression now uses a
real palette-only Player and an actual triangle hit to test Select, Shift-Info and
first-person exclusion; bounds without a Player are not exact geometry.
The subsequent additive Knowledge/Team rebase passed the affected `aomac` tests
(1254 passed, 53 ignored) and another strict workspace/all-target release clippy.

### Recorded live cache and retention comparison (2026-10-16)

The before executable is `4a98542` plus the counting/performance harness; after is
`37166dd` plus the ground/anchor caches, destruction retirement and in-zone model
budget. The upstream delta is the unused `goto` harness correction/documentation.
All measurements were headless, 1280×800, MSAA 1, `AOMAC_PERF=1`, audio muted,
with allocation counting **off** for timings. Live used Aomacfixr, 12 s warmup,
heading `-2.21`, inventory open, and fixed renderer camera
`(924.12, 26.37, -759.85)`, yaw `-2.21`, pitch `-0.32`; Backspace selected actual
walk mode 2 before 60 s idle and 60 s holding W+Left. Both sessions camped cleanly.
The starting avatar positions differed by about 5.6 m and the live population
could change; these are shared-Mac observations, not an isolated causal estimate.
They should not be substituted directly for the earlier, differently viewed
8 ms / 6 ms live profile.

| Workload | Frame p50 before → after (ms) | Thread CPU mean before → after (ms) | Draws at reports before → after |
|---|---:|---:|---:|
| Live idle, 60 s | 4.297–5.569 → 4.579–4.819 | 3.171 → 2.280 | 924–955 → 925–943 |
| Live walking, 60 s | 4.553–5.612 → 4.903–5.051 | 3.477 → 2.575 | 924–930 → 922–938 |
| ICC replay, three-run median | 5.144 → 3.871 | 2.866 → 2.344 | 2307 → 2307 |

Live percentile cells are the range of the individual 600-frame/tail interval
percentiles, **not** a merged whole-session percentile. CPU means are frame-weighted
across all measured intervals (idle 2525/2624 frames; walking 2513/2587).
Live wall means were 5.092 → 4.523 ms idle and 5.215 → 4.816 ms walking.
Replay alternated before/after order across three pairs, 120 warmup + 600 measured
frames per process, with the existing fixed ICC camera. Replay p95 was
5.838 → 5.310 ms. No duplicate character submission or culled-character draw was
found: the own character is excluded from the remote loop, duplicate actor IDs
are rejected, and visible mesh commands are submitted once in their native phase.
The earlier recorded 675-draw replay figure was not reproduced at this baseline;
the current before and after replays both report 2307.

Memory was measured separately with allocation counting enabled. The 500 s before
replay ended only 4 allocations / 505 requested bytes above its first 10 s snapshot,
despite RSS increasing from 742.5 to 778.0 MiB. The after replay ran 72,000 paced
frames (1200 s simulation, 1209.9 s wall) with Rust counters every 600 frames and
native `heap`, `vmmap --summary` and `footprint` snapshots:

| Snapshot (s) | Native malloc blocks | Native allocated heap (MiB) | Physical footprint (MiB) |
|---:|---:|---:|---:|
| 60 | 264090 | 233.302 | 897.816 |
| 600 | 264094 | 233.302 | 898.442 |
| 1140 | 264186 | 233.574 | 898.707 |

After Rust outstanding requested bytes were 228,209,753 → 228,498,906, with the
last 110 s constant apart from one transient 48-byte allocation. Temporary decode
peaks recovered; Metal allocation stayed exactly 428,769,280 bytes. RSS itself
still warmed from 735.8 to 764.4 MiB (last 60 s: 764.16–765.06 MiB), so it is
**not** described as perfectly flat. `vmmap` showed the helper zone's allocated
bytes unchanged at 224.0 MiB while empty/reusable medium-region residency grew;
private dirty size was 225.1 MiB at both native endpoint snapshots. Together with
the nearly constant counted heap/footprint, this identifies resident allocator
high-water warming rather than proportional retained-allocation growth in this
fixed workload. RSS alone would not establish that distinction.

The fixed replay does not cover indefinite encounters or effect churn. Those
separate, genuinely unbounded paths were fixed structurally: monotonically
allocated effect/prop visual histories now retire on destruction, and inactive
CPU/GPU model copies obey the existing budget within a zone. Churn regressions
assert retained-state counts, preserve culled/reappearing actors, and compare
non-vacuous colored RGBA against an equivalent unretired reference. The budget
regression crosses the limit with pinned active models, checks exact evictions,
and prevents queued uploads from resurrecting retired GPU models.

Verification: full workspace release tests passed 2210, 66 ignored; after five
test-only `drain().collect()` assertions were replaced with `mem::take`, 16 affected
tests and fresh strict workspace/all-target release clippy passed. The installed
connector pixel oracle was explicitly run and passed. The counting allocator is
inside a `cfg(test)` module; `nm` found its counter symbols in the test executable
and none in the normal release application, so the normal app has no counting hook.
No real window, desktop input or desktop capture was used.

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

## Deterministic offline graphics fixture

Run the ignored `graphics_offline_fixture` test with installed client assets, no
server or window:

```sh
AOMAC_GFX_OUT="$HOME/aomac-handoff/gfx/my-new-run" \
AOMAC_GFX_SETTINGS=faithful \
cargo test --release -p aomac graphics_offline_fixture -j 2 -- \
  --ignored --nocapture --test-threads=1
```
`AOMAC_GFX_SETTINGS` accepts `faithful` (default, all switches off), `neutral`
(Enhanced boundary with zero-density grass), `shadows`, `ao`, `bloom`, `grass`,
or `all`. Each effect profile uses its explicit default knobs; the manifest records
the full settings value. Selecting a profile does not change the captured world,
camera, clock, actor equipment or lights.


The destination must not exist. The fixed matrix covers ICC 4582 landmark and
platform views at its authored frozen time 2648.69; Borealis 800 street and foliage
with high/low sun and at night; a captured ICC actor at idle time zero; and a
Condemned Subway 127 corridor (indoor lighting needs no day/night duplicate).
ICC's authored frozen GAME clock is never overridden. The actor uses
`zone_ithaca.rec` own-character 25988 appearance/position and the existing Avatar
builder synchronously, without replay worker timing or a synthetic character.
An additional Borealis night view equips that recorded actor with native Light Bar
mesh 26163 on attractor 5, places it at the fixed street position, and includes the
mount's real posed lights. These fixture equipment/placement overrides are explicit
in the manifest; no emissive or other material is replaced.
Every view uses a fixed 1920×1200 camera/lens, day zero, clear weather,
CRT/cloud seed one, zero renderer/effect time, MSAA one and anisotropy one.
Each immutable scene snapshot is rendered by two independently initialized
renderers; raw RGBA8 must match exactly before the run reports success.
The same captured bytes are written as
top-down tightly packed `.rgba` and `.png`, alongside `manifest.txt` and
`status.txt`. A failed/interrupted run is never a pass.

Set `AOMAC_GFX_BASELINE` to an earlier run directory to additionally require exact
raw byte equality against that run. Keep evidence under `~/aomac-handoff/gfx/`,
not in the repository. These are deterministic comparison inputs, not an aesthetic
oracle or a claim of retail screenshot matching.

Effect benchmarks may reuse `graphics_fixture_views` / `graphics_fixture_actor`
and `Offscreen::evidence_timing`. After uploading a snapshot with `frame`, use
`render_frozen` to render its retained camera/actors into the persistent target
without frontend updates, capture allocation or pixel readback. Explicit
`set_gpu_timing(true)` / `read_frame_gpu_ms` provide GPU timestamps; unsupported
hardware returns an error rather than substituting CPU wall time. Ordinary capture
mode does not allocate that target or timing queries.
