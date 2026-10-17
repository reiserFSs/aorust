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

`AOMAC_SHOT_DIR=<dir> cargo test --profile fast -p aomac shots -j 2 -- --nocapture` renders login, character select, the delete dialog and every
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

## Fast development builds and shader iteration

Use `--profile fast` for local tests, fixtures and the live harness. It inherits
release with optimization level 2, no LTO, 256 codegen units and incremental Rust
compilation. `--release` remains the unchanged shipping/gating profile (thin LTO,
one codegen unit); do not compare performance measurements across profiles.

```sh
# Seed a new private target (macOS/APFS; destination must not exist).
mkdir -p /tmp/MyAgent
scripts/seed-fast-target.sh /tmp/MyAgent/target
export CARGO_TARGET_DIR=/tmp/MyAgent/target
cargo test --profile fast --workspace -j 2
cargo test --profile fast -p aomac live_walk -j 2 -- --ignored --nocapture
# Optional windowed development, subject to the live/window lock:
cargo run --profile fast -p aomac -j 2 -- play
# Final shipping gates still use --release, not --profile fast.
```

The warm reference lives at `~/aomac-cache/target-fast` (override with
`AOMAC_FAST_TARGET_CACHE`). Refresh it from a clean worktree of current main,
using the same Rust toolchain and flags as the agents:

```sh
# Only the reference maintainer runs this; never build agents' changes here.
CARGO_TARGET_DIR="$HOME/aomac-cache/target-fast" \
  cargo build --profile fast -p aomac -j 2
```

Do not refresh the reference while another process is seeding it. The seed script
uses `cp -c -p -R` to APFS-clone the complete target, preserving dependency
fingerprints and timestamps. It invalidates only this workspace's `ao-*` and
`aomac` fingerprints: workspace code must rebuild so `env!("CARGO_MANIFEST_DIR")`
and fixture provenance name the new worktree, not the reference checkout. Each
agent then builds only in its private copy; writes are copy-on-write, with no
common Cargo lock or shared mutation. Registry dependencies are reused when
fingerprints match; workspace/native build-script outputs may need rebuilding.
Existing targets are never overwritten. Always run Cargo after seeding before
executing a copied binary.
Refresh is optional when main changes; Cargo rebuilds invalidated artifacts normally.
Use `cargo test --profile fast -p aomac --no-run -j 2` in the reference maintenance
step as well if sharing warm test artifacts is useful.

sccache 0.18.0 was also measured: its Rust key includes the working directory,
so two fresh worktrees had **zero Rust cache hits**, despite sharing the default
disk cache. C/C++ cache hits worked. It is not the dependency-reuse mechanism for
this workflow. Do not use a shared live `CARGO_TARGET_DIR`: parallel revisions
would serialize on Cargo's build lock and invalidate one another's artifacts.

### Measured iteration costs (2026-10-17)

Single-run `/usr/bin/time -p` wall seconds on this Mac, `-j 2`, private targets;
before source `82bed1e`, after this iteration change. Shipping release settings
were identical before/after. These are build/fixture latency measurements, not
Enhanced frame-time or retail visual-quality claims.

| Work | Before | After |
| --- | ---: | ---: |
| Clean `cargo build -p aomac` | release 259.79 s | fast 211.86 s |
| Rebuild after `touch crates/ao-render/src/grade.wgsl` | release 259.68 s | fast 4.85 s |
| First fixture test executable build after app build (`--no-run`) | release 286.21 s | fast 77.92 s |
| Full faithful fixture, release, 14 views / 28 independent captures | 19.14 s | 13.07 s (final merged source) |
| Selected faithful fixture, release, `borealis-foliage-sun-high` | 6.03 s | 5.22 s |
| Scene loads for full matrix | 14 | 6 (776.807 ms total loader time) |
| Shader-only edit using existing executable + runtime sources | release rebuild required | **zero Rust compilation**; final release Enhanced-all selected-view run 4.90 s |

The exact-view filter already existed; no redundant filter was added. The full
fixture now shares snapshots without sharing renderers or weakening repeats.
The fast full faithful fixture also passed in 22.16 s; fast is not promised to
render faster than release. Runtime fixture latency varies with machine load;
the reliable shader-iteration win is removing compilation.

Two agents built fresh private worktrees simultaneously. With sccache alone they
took 174.43 / 173.73 s and had zero Rust hits. With provenance-safe APFS seeding,
cloning took 1.81 / 1.32 s and builds took 127.81 / 149.21 s. Both seeded builds
compiled **only the nine workspace crates**, with no registry dependencies rebuilt
and no shared Cargo target lock. The script also refused an existing destination
without deleting it and rejected a missing reference without creating a target.

All 14 faithful views were byte-identical to the pre-change release baseline,
including independent repeats, in both fast and release. All 14 runtime-loaded
faithful views matched that baseline too; the Enhanced-all selected-view runtime
override matched its embedded-source capture byte-for-byte. These comparisons
also passed on the final merged grass/HDR/contact source: full faithful 13.07 s,
Enhanced-all embedded 5.50 s and runtime-loaded 4.90 s. Evidence directories are
`~/aomac-handoff/gfx/iterspeed-*` (plus `iterspeed-enhanced-*`).
After rebasing over the HDR/contact/grass renderer updates, headless release
workspace gates passed again: **2264 passed, 73 ignored** and strict
`cargo clippy --release --workspace --all-targets -j 2 -- -D warnings`.
The final conflict-free rebase over grass commit `a011137a` then passed affected
`ao-render`/`aomac` tests under `--profile fast` (160 + 1328 + 1 passed), plus strict
workspace/all-target fast clippy, before publication.

### Runtime shader overrides

All file-backed WGSL/Metal sources use the override, including the world/Enhanced
(receiver helpers and AO sun-share cap in `enhanced_output.wgsl`), grade, grass
(including `grass_shadow.wgsl`), shadow, contact shadow, AO and bloom
(including `bloom_glare.wgsl`) paths.

For shader-only iteration, build the fixture test once, then set
`AOMAC_SHADER_DIR` to the **absolute** `crates/ao-render/src` directory in your
worktree. Each new renderer reads its shader sources from that directory instead
of the embedded strings. Edit WGSL, then rerun the already-built test executable
directly: running Cargo would still notice the embedded source changed and rebuild.
The override directory must contain every source used by that run; missing or
unreadable files fail loudly rather than silently falling back. Unset the variable
for shipping/default embedded behavior. This is reload-on-renderer-creation, not
live pipeline replacement in an existing window.

```sh
cargo test --profile fast -p aomac graphics_offline_fixture -j 2 --no-run
# Cargo prints the test executable path; use that exact path, not a wildcard.
AOMAC_SHADER_DIR="$PWD/crates/ao-render/src" \
AOMAC_GFX_OUT="$HOME/aomac-handoff/gfx/shader-iteration-unique" \
AOMAC_GFX_VIEW=borealis-foliage-sun-high AOMAC_GFX_SETTINGS=all \
AOMAC_GFX_PNG_ONLY=1 \
  /absolute/path/printed/by/cargo/aomac-<hash> \
  graphics_offline_fixture --ignored --nocapture --test-threads=1
```

## Deterministic offline graphics fixture

Run the ignored `graphics_offline_fixture` test with installed client assets, no
server or window:

```sh
AOMAC_GFX_OUT="$HOME/aomac-handoff/gfx/my-new-run" \
AOMAC_GFX_SETTINGS=faithful \
cargo test --profile fast -p aomac graphics_offline_fixture -j 2 -- \
  --ignored --nocapture --test-threads=1
```
`AOMAC_GFX_SETTINGS` accepts `faithful` (default, all switches off), `neutral`
(Enhanced boundary with zero-density grass), `shadows`, `ao`, `bloom`, `grass`,
or `all`. `all-no-bloom` differs from `all` only by disabling bloom, for highlight
washout comparisons without changing shade, AO, grass or exposure. Each effect
profile uses its explicit default knobs; the manifest records
the full settings value. Selecting a profile does not change the captured world,
camera, clock, actor equipment or lights.

`AOMAC_GFX_VIEW=<exact-name>` filters before scene loading and rendering; this
existing filter renders only that view (including its two independent repeats).
Within one process, views share immutable scene/collision snapshots keyed by exact
playfield and time bits, with the fixture's fixed clear weather/day. The current
17-view matrix loads nine snapshots instead of seventeen; renderers, uploads and
independent-repeat checks remain separate. `scene_load_count` and `scene_load_ms`
in the manifest report loader work (collision loading is not included in the time).
The destination must not exist. The fixed matrix covers ICC 4582 landmark and
platform views at its authored frozen time 2648.69; Borealis 800 street and foliage
with high/low sun and at night; a captured ICC actor at idle time zero; Newland 566
at midday/dusk/night; and a Condemned Subway 127 corridor (indoor lighting needs no
day/night duplicate). ICC's GAME interface freezes its sky at 2648.69 even when
the native clock changes: earlier `icc-*-day/dusk/night` captures are **frozen-day**
evidence, not night proof, and those six redundant variants are no longer captured.
Actual daylight/night fairness is checked with Borealis and Newland's live native clocks.
Newland's street eye `[414, h + 2.5, -320]` and target `[440, h + 1, -335]`
resolve their heights independently through native `Collision::terrain_height`.
For the installed snapshot this is eye `[414, 27.5, -320]`, target `[440, 26, -335]`.
The earlier fixed-y treetop view and the grounded x=410 view obscured by a nearby
tree are iteration evidence only, not camera-matched final comparisons.
ICC's original `*-authored` views retain their authored frozen GAME clock. The actor uses
`zone_ithaca.rec` own-character 25988 appearance/position and the existing Avatar
builder synchronously, without replay worker timing or a synthetic character.
The manifest records the native authored `GAME.CurrentDayIntensity` as `daylight`;
Enhanced exposure uses this value directly, never a brightness-based day/night guess.
Two Borealis night views equip that recorded Testy actor with native Light Bar
mesh 26163 on attractor 5, place it at the fixed street position, and include the
mount's real posed lights: `borealis-lightbar-night` and
`borealis-lightbar-night-close` (camera `[679.6, 74.0, -480.2]`, looking at
`[679.6, 74.0, -482.7]`, 2.5 m from the upper-body target / 2.77 m from actor origin).
These fixture equipment/placement overrides are explicit in the manifest;
no emissive or other material is replaced.
Every view uses a fixed 1920×1200 camera/lens, day zero, clear weather,
CRT/cloud seed one, zero renderer/effect time, MSAA one and anisotropy one.
Each immutable scene snapshot is rendered by two independently initialized
renderers; raw RGBA8 must match exactly before the run reports success.
The same captured bytes are written as top-down tightly packed `.rgba` and `.png`,
alongside `manifest.txt` and `status.txt`. Set `AOMAC_GFX_PNG_ONLY=1` to retain
only PNG images (including repeat-failure images); byte-exact independent-repeat
checks still run in memory, and baseline comparison then decodes the baseline PNG.

**Known open strict-repeat failure (2026-10-17, frozen source `6031e9c7`):**
LookLand's faithful and neutral matrices pass all seventeen views exactly, with
neutral versus faithful zero differing bytes. The `all` and `all-no-bloom` profiles
both fail `icc-actor-authored` at pixel `(1599, 490)`: first RGB `(23, 40, 19)`,
repeat `(22, 41, 19)` (two channel bytes, maximum one code; blue/alpha unchanged).
The root is not isolated; disabling bloom does not remove it. The historical
`9f78245e` all-before capture also failed this view, at a different pixel
`(1041, 463)` (red `53 -> 54`, blue `34 -> 33`); that does not establish the
same mechanism. The original failed manifests and repeat images are retained in
`gfx/v3/LookLand/after` and `after-no-bloom`. Remaining views were captured once
in `after-remaining/<view>` and `no-bloom-remaining/<view>` rather than retrying
the failed view. The first images are also collected into the flat comparison
directories for the final sheets. No tolerance, retry-to-pass, or passing status
was added. Main approved recording this open item instead of extending the
investigation beyond its ten-minute limit.

**Night fairness policy — root lighting equality, visible night blades:**
All five native night snapshots report `daylight=Some(0.0)`. Historical per-pixel measurements, with bloom disabled,
showed foliage and Newland positive energy beyond one display code (`gfx/v3/LookLand/metrics.txt`, source `6031e9c7`):

| Night view | profile | Mean positive excess above 1 code | Maximum positive Y |
|---|---|---:|---:|
| Borealis foliage | all-no-bloom | 0.057036 | 19.1480 |
| Newland | all-no-bloom | 0.153212 | 62.5596 |
| Borealis foliage | grass only | 0.131737 | 20.2162 |
| Newland | grass only | 0.548432 | 80.8030 |
| Borealis street / light bar / light bar close | all-no-bloom | 0 | 0.7192 |

Those historical numbers use Rec.709 luminance of sRGB byte codes and the per-pixel metric
`mean(max(0, Y_after - Y_faithful - 1))`; they are not measurements of the current block-mean criterion.

Night grass uses native terrain-corner lamp evaluation, saturation and interpolation at its root, with a
post-fog native-root bound. The remaining brightness mismatch comes from magnifying a full-resolution root
texel over a different covered ground pixel, not incorrect palette composition or gamma.
Night blades therefore use component-wise Min blending against the completed native ground, preserving
that pixel's mip, palette, lighting, fog and two-pass quantization without a framebuffer copy.
The Min pass keeps depth testing but disables depth writes: native foliage and statels drawn later must not
be rejected behind blade depth, which would otherwise hide darker native fragments and brighten Borealis.
A stable per-blade hash partitions normal and Min passes; the Min fraction is
`1 - smoothstep(0.0, 0.25, ambient_g.w)`, with lighting and the root bound interpolating over the same range.
Only Min runs at authored night, only normal at full daylight, and both at twilight; blade population is unchanged.
Night blades remain visible geometry rather than being removed by a daylight fade.

The ignored, headless `graphics_offline_night_is_never_brighter_than_faithful` regression captures
`borealis-foliage-night` and `newland-night`, each with `grass` and `all-no-bloom`, against the same view's
`faithful` render. At fixed 1920×1200, MSAA one and bloom off, it partitions the full image into aligned,
non-overlapping 16×16 blocks. For every block it requires
`mean(Y_enhanced) - mean(Y_faithful) <= 1` code, with
`Y = 0.2126 R + 0.7152 G + 0.0722 B` on encoded sRGB byte values (not linearized RGB).
No extra tolerance, failing-block allowance, or global averaging is applied.
`night-brightness.txt` and stderr report each view/profile's maximum signed block delta, worst block's top-left
pixel coordinate, and count of blocks above one code. All four comparisons and their PNGs are retained before
a brightness failure is returned; `status=pass` is written only when every comparison passes.

Measured 1920×1200 block regression, `e4d3881b` grass baseline → final fix:

| Night view | Profile | Maximum block delta before → after (Y codes) | Blocks above 1 before → after |
|---|---|---:|---:|
| Borealis foliage | grass | 7.681262500 → 0 | 401 → 0 |
| Borealis foliage | all-no-bloom | 5.968861719 → 0.057929688 | 43 → 0 |
| Newland | grass | 20.947036719 → 0 | 854 → 0 |
| Newland | all-no-bloom | 10.089657812 → 0.019624219 | 66 → 0 |

All seventeen final `grass` fixture views had zero differing bytes between independent repeats.
Against `e4d3881b`, all twelve non-night views and three night street/light-bar views were byte-identical;
only `borealis-foliage-night` and `newland-night` changed. These fixture measurements are not a release-gate claim.

Run with the installed client data and a new output directory; no application window or live login is needed:

```sh
AOMAC_GFX_OUT=/tmp/night-block-regression \
  cargo test --profile fast -p aomac graphics_offline_night_is_never_brighter_than_faithful -j 2 -- --ignored --nocapture --test-threads=1
```

The earlier daylight-gated captures in `gfx/v3/NightGrass-fix/metrics.txt` made grass byte-identical to faithful
by removing night blades. They are historical evidence only, not acceptance of root-lit visible night grass.

Bloom's approved uniform authored lens halo is also separate: with bloom enabled,
street/light-bar/close mean changes become +0.691265/+0.699451/+1.169199 codes;
Newland remains -2.103241 and foliage -2.080203. The maximum positive pixel change
is 64.8456 codes including quantization/chroma. No new night HDR highlight seeds
are admitted. Exact per-view measurements and first/repeat evidence remain in
`gfx/v3/LookLand/metrics.txt`, `acceptance-summary.txt` and the final sheets.

For sky precision investigations, `AOMAC_SKY_PIXELS='743,208;779,212'` also
writes `<view>-sky-first.txt` and `<view>-sky-repeat.txt` from each renderer's
last uploaded SingleCloud layer17 draws. These opt-in reports include unblended
source RGBA float bits, sampled alpha, interpolation and runtime cloud vertices;
they do not alter the strict repeat comparison or retry a capture.
The ignored renderer test `native_cloud_precision_four_sample_gpu_timing` measures
faithful ICC platform rendering at **actual 4× MSAA**, rejecting sample-count
clamping. Run alone with exclusive GPU access and
`AOMAC_GFX_SKY_TIMING_OUT="$HOME/aomac-handoff/gfx/v3/LookV3/sky-4x-<new-run>"`.
Its explicit test-only variants compare the old four grouped submissions,
thirty native submissions in the original pass, and thirty native submissions
in a beauty-only sky pass before world work. Shader math and scene inputs remain
identical; only sky output declarations/pass layout differ. It retains all three first-frame
PNGs, raw CPU/GPU samples, p50/p95/p99 and the GPU median delta against the 0.1 ms
budget. Each variant receives 120 warmup and 600 measured frames in ABC/CBA order;
these are timings, not retries or a substitute for independent-repeat acceptance.
A failed/interrupted run is never a pass. For close-up bloom evidence without raw
RGBA files, select the view explicitly:

```sh
AOMAC_GFX_OUT="$HOME/aomac-handoff/gfx/lightbar-night-close-bloom" \
AOMAC_GFX_SETTINGS=bloom AOMAC_GFX_VIEW=borealis-lightbar-night-close \
AOMAC_GFX_PNG_ONLY=1 \
cargo test --profile fast -p aomac graphics_offline_fixture -j 2 -- \
  --ignored --nocapture --test-threads=1
```

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

### Enhanced CPU encode/submission benchmark

Run before and after a production optimization with the **same harness** and
different, nonexistent output directories (installed assets required; no server
or window):

```sh
AOMAC_GFX_OUT="$HOME/aomac-handoff/gfx/enhanced-cpu-before" \
cargo test --profile fast -p aomac graphics_enhanced_cpu_timing -j 2 -- \
  --ignored --nocapture --test-threads=1
```

Repeat with `enhanced-cpu-after` after the production change. The default view is
`borealis-foliage-sun-high`; optionally set `AOMAC_GFX_VIEW` to another existing
`borealis-foliage-*` view, identically for both runs. This reuses the
`graphics_offline_fixture` view, lens, clear weather, seed, snapshot loading,
collision exposure and procedural grass warmup. One immutable scene is loaded
once and cloned into fresh renderers in **all, faithful, faithful, all** order.
Each phase has 120 warmup frames and 600 measured frames at 1920×1200,
single-sample/anisotropy 1, renderer time and dt zero. There is no GUI draw,
pixel readback or PNG encoding. Frozen rendering retains renderer-owned
render-count animations, as the existing evidence harness does; it never
advances the frontend or scene clock.

`frames.csv` contains paired GPU frame timestamps and CPU wall spans per measured
frame; `summary.csv` reports per-phase mean and nearest-rank p50/p95/p99.
Compare both paired occurrences of each profile, not just the first all/faithful
pair. `manifest.txt` records revision, source diff, resource hash, camera, lens,
settings and resource counts; `status.txt` distinguishes complete/failed runs.
GPU timestamp resolution/readback serializes frames **after** the CPU sample:
these are isolated submission measurements, not pipelined live FPS.

Explicit `set_cpu_timing(true)` on `Renderer` or `Offscreen` enables
`frame_cpu_timings()`; the default performs no clock reads or timing allocation.
Instrumentation boundaries (milliseconds, CPU **wall**, not thread CPU):

- `prepare_ms`: entry to `Renderer::render` through preparation immediately
  before command-encoder creation, including grass preparation.
- `encode_ms`: command-encoder creation through `finish()`, including existing
  interleaved CPU work/uploads, world draws and Enhanced stages.
- `finish_ms`: only `enc.finish()` wall time, nested within `encode_ms`.
- `queue_submit_ms`: only `queue.submit` wall time, excluding GPU completion
  waits, timestamp resolve/readback, frontend updates and GUI submission.
- `cpu_render_ms`: sum of those three spans.
- `shadow_prepare_encode_ms`: the existing `encode_enhanced_shadows` call,
  **combined preparation and encoding**, kept in its original ordering.
- `grass_prepare_ms`: `prepare_enhanced_grass`; `grass_encode_ms`: the grass
  draw-call encoding inside the world pass.
- `ao_ms` and `bloom_ms`: their individual encode calls (zero when absent).

Stage spans are nested diagnostics, not additional time to add to the total.
Depth resolution, compositing and grass shadow-receiver preparation remain in
the parent encode span rather than being mislabeled as isolated AO/grass work.

Static sun-caster instance ranges preserve draw and instance order while
coalescing adjacent slots for the same draw; cascade binding state changes only
when its pipeline, mesh or material changes. The union of the original cascade
admission tests is retained: existing instance spheres are not guaranteed
conservative after arbitrary shear or repose, so narrowing each cascade's list
from those bounds would not establish pixel identity. Primary actor-admission
draws retain their original order and draw granularity. World/actor sun receiver
bindings are shared across pipeline changes and restored at world-phase entry
after grass uses its own receiver layout.

## Optional material-ground grass

`AomacGroundGrass` is an off-by-default enhancement, not a reproduction of native
vegetation geometry. Every green part of the terrain's original tile pictures
admits blades, including grass/soil transition tiles; the per-texel green mask
keeps soil, roads and rock bare. Liquid-overlapped cells, room floors and covered
ground remain excluded. Source-triangle liquid bounds conservatively omit some
shoreline grass. The GPU generates blades from a fixed world-cell hash, with a
16384-cell budget and no per-blade CPU records, texture atlas or external artwork.
The design follows ao-vk's procedural grass (MIT, attribution in `THIRD_PARTY`):

- *Blades*: three-section camera-facing strips (four triangles) about the blade's own
  axis, so they are never edge-on, arching over towards a pointed tip. Four kinds:
  grass blade, broad blade, seed stalk (straw head) and flower (heads in patches of
  one colour, a few strays). Every kind, flower and seed head included, is at most
  0.5 m tall, so grass cannot hide a target.
- *Placement*: blades of a cell come from a low-discrepancy sequence, so any prefix of
  them is evenly spread. The density knob sets how many of the 1536 candidates a cell
  can hold are drawn (default 1.0 = 768 wider blades; at most 2.0). They then thin out between
  tufts (a world-anchored jittered lattice of centres, the blades fanning out from
  the centre), in slow clump and dry patches, on near-vertical faces (from 60°, none past
  75° of the interpolated native normal), and towards the green mask's edges and
  neighbouring soil/path cells (blades thin and shorten over 1.2 m).
  The slow fields are baked once into a tileable 256²
  two-layer RGBA8 noise texture (`grass_noise.rs`, 128 KiB, 64 m period), never per frame.
- *Distance*: eight instancing bands bound the work to the stable blade-rank prefix
  that can survive in each cell. Beyond 12 m density falls as
  `1 / (1 + ((distance - 12) / 10)²)`; survivors widen into sparse tufts.
  Rank changes shrink blades smoothly, and the outer 30% of the field also fades
  with hardware alpha-to-coverage dithering under MSAA (single-sample retains geometric fade).
  From 24–80 m the colour converges on the root's native terrain texel, removing
  dark tuft speckles and the carpet-to-terrain seam. Subpixel blades widen to 0.8 px and blend towards the
  ground rather than flickering. Cells outside the view frustum are culled on the CPU.
  The two nearest bands retain curved six-vertex blades. More distant bands use
  root/tip triangles; geometry morphs to this straight profile and head colour
  fades out from 22–27 m, before the instancing switch, so neither can pop.
- *Wind*: waves running through the field along the native weather wind, a cross
  wave, each blade's flutter, and gust bands sweeping over it (the bent blades lighter).
  The native wind scales the lean; a light base breeze (the strength of a 0.35 bend)
  keeps calm weather from freezing a field (a deliberate Enhanced addition).
  Wind direction is the native wind's, or a fixed one when calm.

Slope uses actual triangle gradients, not the direction-dependent corner height
range; a cell is rejected only where both triangles exceed about 75°. The default
distance is 160 m (bounded to 180 m), further limited by the original projection
far plane and fixed cell budget, not by the terrain's fully-fine radius.
The registered Graphics slider uses the same 160 m default and 180 m maximum.
Explicit saved preferences are preserved; set **Ground Grass Distance** to 160 m
or reset that setting to adopt the new default if an older 50 m value was saved.
This is an artistic Enhanced addition, not a reconstructed retail plant
distribution. Collision and selection are unchanged.

Native vegetation remains untouched and does not exclude the grass carpet.
Blades extend over coarse terrain too: on terrain-selection changes their root
heights and cell diagonals are projected onto the triangles actually emitted by
the native terrain LOD. The collision exposure and underwater decisions still use
the original terrain heights, so changing rendered LOD cannot poison the exposure
cache. No terrain refinement, collision or faithful rendering is changed.

Native substrate alone is not proof that blades can reach the visible surface:
statel floors may overlay a grass cell. An opt-in callback reuses the loaded native
`Collision` ground query from terrain height + 0.5 m (the blade-height cap).
A higher floor at any of five cell samples rejects the whole cell, including
fully floor-covered cells. Overhead arches and shelters above the blade envelope
do not exclude green terrain. This is independent of sky-fill proof: only
collision-baked terrain vertex alpha can authorize sky fill, and roofs remain
closed. No collision capability means no grass.
Exposure is computed only on generation, at most four new cells per frame,
and retained in a 32768-entry FIFO cache tied to the immutable loaded collision
source. Door flags affect actor LOS immediately but do not flush static exposure.
Nearest unknown cells are generated first; unclassified cells remain absent until
checked. Candidate selection is one pass over native cells and is reused until
the native terrain selection/radius changes. Stationary warmed frames upload only
the frustum-visible cell list and wind/bender uniforms. Blades themselves are expanded entirely by the GPU.

Colour samples the original oriented terrain texture and native ground palette
at each blade's root. A blade is the meadow's green (mostly one hue, a little of
the texel's own, straw in dry patches, each blade a shade off) at the texel's
luminance, the root growing out of the texel itself and a root-to-tip gradient and
canopy darkening that average one over the blade, so a field is as bright as the
ground it stands on. Where the sun lights the ground the blades may rise to a
meadow luminance of 0.42 if the texel is duller (the daylight policy). This lift
requires native `GAME.CurrentDayIntensity` and raw sun visibility before the
artistic retained-shadow floor; night and roof umbra cannot qualify. Native fog
still attenuates the resulting surface radiance.
Authored night retains visible blades rather than fading their geometry away.
Night blade lighting follows native terrain-corner lamp evaluation, saturation
and interpolation at the root, with a post-fog native-root bound. Component-wise
Min blending against completed native ground also prevents a magnified root
texel from brightening a different covered pixel; native palette and gamma
composition are unchanged. Complementary normal/Min passes partition blades by
a stable hash, with Min fraction `1 - smoothstep(0.0, 0.25, ambient_g.w)`.
Lighting and root-bound strength interpolate over the same range. Only one pass
runs at day/night endpoints, both at twilight; no blade population is removed.
The Min pass depth-tests without writing depth, allowing later native foliage
and statels to draw rather than rejecting their darker fragments behind blades.
Regression:
`AOMAC_GFX_OUT=<new dir> cargo test --profile fast -p aomac
graphics_offline_night_is_never_brighter_than_faithful -- --ignored --nocapture --test-threads=1`
requires every aligned 16×16 block's mean Rec.709 sRGB-code luminance to exceed
faithful by at most one code for the `grass` and `all-no-bloom` profiles at
`newland-night` and `borealis-foliage-night`; no additional tolerance is allowed.
Far roots also converge on the tile's 8×8 mip, matching native coarse composites
rather than magnifying one full-resolution dark texel with a derivative-free UV.
Normals use the native central height differences, cached at tile generation. The
shader evaluates the original four terrain-corner sun Gouraud values and
interpolates them on the native diagonal, including per-corner saturation and
ambient for the simple-ground path. Daylight retains point and spot lighting
evaluated once at the root; night evaluates these lamps at the native corners
before saturation and interpolation. Fused and two-pass fog composition retain
their native equations, and fog is
at least the root's native fog, so a raised tip never reduces extinction. The sun
also lights a rounded blade's near edge and shades its far one (zero-mean), shines
through its thin upper part when the sun is behind it, and a dense canopy keeps a
low sun off the lower blades.
Enhanced sun shadows reuse the world's current-frame cascades, geometric receiver
plane correction, PCF and cascade transitions at the blade's root on the ground
normal. Like native ground, the shader removes the direct sun surviving corner
saturation before adding/clamping the baked palette; local and baked lights are
never shadow-multiplied. The HDR sun mask uses that same pre-palette share and the
terrain's Rec.709 luminance weights, avoiding darker blades on HDR-brightened ground.
No authored sun or zero shadow strength leaves native light intact.
AO reuses the native surface's canonical sun/sky receiving budget, bounded by
the blade's beauty colour and attenuated by the native fog equation (both factors
for two-pass fog). Receiving follows the 22–27 m geometry morph; casting drops
earlier within that morph, depending on MSAA coverage. Half-resolution AO must
not treat subpixel root/tip tufts as solid dark occluders.
The existing ambient attachment's alpha encodes porosity; its negative far tag
survives even one covered sample at 8× MSAA. AO alone ignores fully porous depth
and weights partially porous taps; beauty/contact-shadow depth and their native
opaque-object occlusion are unchanged. Mixed MSAA edge pixels are conservatively
porous for AO only. No new render targets are allocated, and
grass-off AO does not load porosity metadata. This is an Enhanced treatment,
not retail behaviour.
Wind uses the current native weather wind plus the light base breeze above.

Interaction follows the approved conservative rule: own-character feet always
bend grass, including first person. Other sources must be currently drawn opaque
primary character bodies within 15 m whose feet + 5 cm are inside the final render
frustum and clear the final camera's existing bidirectional collision/closed-door
line-of-sight query. Admission uses the current prepared draw list, including body
fade/priority, before selecting the fifteen nearest remote sources. Camera shake
or a renderer-rejected body therefore cannot evict a visible source at the cap.
Remote admission also rejects a body bound whose nearest view-space depth is at
or beyond the final camera fog end, after local fog evaluation. Rigid and current
CPU-skinned geometry use their transformed local AABB, without the animation
margin. Native GPU CAT poses retain the conservative bind sphere plus pose margin;
this bound can straddle fog even when every actual posed vertex is fully fogged,
so suppression of every such animated body remains unresolved (no CPU re-skin or
GPU readback is introduced). A partially unfogged bound remains eligible; disabled fog,
native fog-disabled body materials and rendering effects 2/4/5/6 preserve their
native exemptions (`shader.wgsl::shade`, Randy `10041876` vertex-depth fog).
Own-character interaction is unchanged. This only filters Enhanced benders;
faithful actor rendering is untouched.
CPU candidate allocations are exchanged with the host and reused; their size is
bounded by the currently submitted character frames, while the GPU stays fixed
at sixteen sources. Missing collision admits only own-character interaction.
Repeated renders of the same actor snapshot preserve bends for screenshots;
the next actor batch replaces all candidates, including an empty batch.
No recovery grid or actor history exists; concealment, disappearance or a blocked
ray removes the bend immediately. Grass overwrites the optional AO attachment
with its own receiving contribution rather than inheriting the terrain beneath
it. Its emissive attachment remains zero: grass never seeds bloom.

Occlusion limitation: high building floors above the 0.5 m blade envelope can hide
otherwise admitted terrain. Opaque floors prevent visible leaks, but the hidden
carpet still incurs vertex/fragment/draw work before those floors overwrite it.
The ICC platform negative control measured about 0.72 ms paired whole-frame GPU
contribution at 1920×1200, true 4× MSAA (0.67/0.77 ms in reverse-order rounds).
There is deliberately no camera-history occlusion or CPU LOS heuristic.
Grass precedes priority-3 opaque statels in the native pass, so current-frame Hi-Z
requires an opaque-depth prepass plus conservative MSAA depth reduction; moving
grass after native priority/depth-clear commands is not a safe substitute.


Effect buffers are fixed-size and released on disable; all-off execution does
not prepare cells, upload grass data or create a grass pipeline. Regression tests
cover green-texel admission, exact roots on emitted coarse/fine terrain, stable density, submerged placement, fully fogged
body admission (with visible, partial-body and fog-exempt controls), and an actor
behind a synthetic wall producing exactly the same bender list as its absence.
`grass_pixels_receive_sun_shadows_and_ao_without_emission` is a headless synthetic
native-cell regression: density-zero/one renders identify actual blade pixels
before asserting shadow and AO darkening, neutral zero-AO/no-sun paths, and zero
emission. It is not retail-image or native-collision visual acceptance.

The ignored `grass_offline_capture_and_timing` fixture compares density-zero
neutral frames with grass using `FrameGpuTimer`, in both orderings. Alongside the
canonical ICC/Borealis/Subway views it derives close ground views from exposed
native grass cells, recording their texture, grid and camera rather than assuming
that canopy-looking cameras show terrain. Its manifest includes per-tile material,
exposure and projected-root rejection counts, drawable batch availability,
effect buffer bytes and native Metal allocation deltas. The timed GPU result is
the paired whole-frame contribution, not an isolated draw timestamp.
Set `AOMAC_GFX_VIEW` to select a single view and `AOMAC_GFX_PNG_ONLY=1` to omit
raw image files. `icc-coastal1` and `icc-coastal2` use scene cameras
`[660, 24.3, -714]` and `[700, 24, -710]`, both yaw 0 / pitch −0.3;
`borealis-grass-ground-day` selects the exposed-ground close-up. Each ordering
prepares the static exposure cache explicitly (wall time recorded separately),
then warms 320 frames before measuring 600 frames. This is a stationary GPU cost,
not a streaming-generation latency claim. Coastal green ground is grass-bearing
even where native vegetation already exists.

For the all-effects 1920×1200 timing matrix, set `AOMAC_GFX_EFFECTS=1`
(sun shadows, contact shadows, AO and bloom), `AOMAC_GFX_HDR=1`, and
`AOMAC_GFX_MSAA=4`. The grass views include `icc-grass-ground-mid-day`,
`newland-grass-ground-day`, and `borealis-grass-ground-far-day`.
Newland day captures use time 3240 (midday), while preserving the original
4700-derived camera selection. `AOMAC_GFX_DAY_TIME` overrides Newland's daytime
only; `AOMAC_GFX_DISTANCE` overrides the grass radius without changing density.

Measured on Apple M4 Pro at code revision `afb37e44`: 1920×1200, actual 4× MSAA,
all five effects enabled, density 1, distance 160 m, sun boost 0.5 and grade 0.8.
Values below are grass-on minus grass-off whole-frame GPU p50, in milliseconds;
both variant orders are retained rather than selecting the faster order.

| View | Off/on order | On/off order | Pair mean |
| --- | ---: | ---: | ---: |
| Newland park, noon 3240 | 2.258 | 2.262 | 2.260 |
| ICC mid / arch, native frozen time | 1.766 | 1.677 | 1.722 |
| Borealis full-green hillside close | 1.827 | 2.279 | 2.053 |
| Borealis hillside mid/far overview | 1.106 | 1.599 | 1.352 |

This is approximately the 2 ms budget, not a strict ≤2 ms guarantee: the Newland
pair is 0.26 ms above that nominal target. The older fixture's advertised 4× runs
actually rendered at 1× because the persistent timing target overwrote the host's
sample count each frame; `evidence_timing_honors_explicit_msaa_across_frames`
regresses the corrected behavior. Before/after PNG sheets are retained under
`~/aomac-handoff/gfx/v2/GrassV3/`. Newland's baseline is late afternoon 4700, not
a same-time lighting comparison; the other comparison cameras/times are pinned.

### Far-grass speck regression

`grass_offline_capture_and_timing` now checks `borealis-grass-ground-far-day`
against its same-run grass-OFF frame. In the fixed native-pixel hill ROI
`[550,300,1400,600)`, a 7×7 local Rec.709-weighted encoded-RGB median detects
pixels below 60% of that median with a drop exceeding 0.06. At least 40 of the
49 paired-OFF neighbours must be green (`G > 1.15R` and `G > 1.15B`), excluding
structures, rocks and water. NEW outliers may occupy at most 0.05% of the
eligible terrain; the old AO model produced about 3.4%. This is a spatial
contrast regression, not a linear-light radiometric claim.

For shader-only iteration, `AOMAC_GFX_CAPTURE_ONLY=1` retains the explicit
exposure warm and 320 frozen frames, but skips measured frames and reverse order;
GPU timings are explicitly unexercised. `AOMAC_GFX_AO`, `AOMAC_GFX_SHADOWS`
(sun), and `AOMAC_GFX_CONTACT` accept `0`/`1` to override the all-effects preset
individually. `AOMAC_GFX_ASSERT_SPECKS=0` is only for counterfactual diagnosis;
normal acceptance enforces the speck limit.

The controlled 4× capture for frozen speck unit `6031e9c7` (12 m thinning scale)
on the native-DAY/shared-AO base `2e86c663` restores only the obsolete grass AO
formula and opaque AO alpha for its BEFORE frame.
The grass-OFF RGB frames are byte-identical. BEFORE has 471 new outliers
(610 total; paired OFF 163); AFTER has 36 new outliers (190 total), over
252,301 eligible pixels, below the 127-pixel limit. Restoring the old formula
fails the actual regression; the corrected shader passes.

Quiet Newland timing for the frozen speck-fix unit `6031e9c7` (12 m thinning scale), on the same M4 Pro
at 1920×1200 and actual 4× MSAA with all five effects: grass-on minus grass-off
GPU p50 is 2.231 / 2.097 ms in opposite orders (mean 2.164 ms). Prepare p95 is
0.049 / 0.045 ms. These use the newer native-DAY defaults: shadow strength 0.45,
sun boost 0.6, grade 0.8 and contact strength 0.8; they are not a controlled
performance comparison with the older `afb37e44` table above, nor a claim for the current 10 m scale.

The 10 m thinning scale preserves every blade within 12 m and keeps the same
160 m radius; only distant rank density changes, with surviving tufts widened.
On the same settings/hardware, opposite-order GPU p50 contributions are
1.862 / 1.991 ms (mean 1.926 ms), with prepare p95 0.052 / 0.049 ms.
The 22–27 m geometry/head-colour morph finishes before band 2's nearest-edge
threshold of about 29.3 m. The far-hill speck regression still passes:
30 new dark outliers over 252,301 eligible pixels (paired OFF 163, ON 185).
These are paired whole-frame GPU medians, not an isolated grass draw timestamp
or a guarantee about tail latency.

