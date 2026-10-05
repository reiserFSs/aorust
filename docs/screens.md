# Original login-flow screens (login → progress → character selection → loading)

Faithful spec of what the original Anarchy Online client (Project Rubi-Ka build, `version.id` 00.7.2_EP1) shows between
process start and entering a zone. Everything here was read from the client's own data (`cd_image/gui`, `cd_image/text`,
`cd_image/rdb.db`, `CharCreateCamera.dat`) or from Ghidra decompiles of the original 32-bit DLLs. Loaders for the data live in
`crates/ao-formats/src/screens.rs`. Anything not proven is labelled **UNRESOLVED** / **[INFERENCE]** — nothing below is a guess
presented as fact.

Evidence tags: `[GUI 0x1001…]` = `GUI.dll` (PE image base 0x10000000, **all** screen code lives here, not in
Interfaces/Gamecode), `[GC …]` Gamecode.dll, `[IF …]` Interfaces.dll. Function names are the DLL's own RTTI/mangled names.
Ghidra project: `/tmp/aomac-ghidra/dsky/gui` (program `GUI.dll`); helper scripts used are listed at the end.

## 0. Who owns what

* `AnarchyOnline.exe` (the game; 80 KB, PRK-patched) is started by the **external PRK launcher** with the command line
  `IA<u32 ip> IP<port> UI` (see `docs/protocol.md` §1). The original stock launcher `Anarchy.exe` (dimension combo box,
  `data/launcher/dimensions.txt`, `AnarchyLauncher.url`) is *not* part of the in-game flow in this client.
* The in-game login screens are `LoginModule_c` [GUI 0x10012614] (state machine), `LoginWindow_c` [0x10015331],
  `ProgressWindow_c` [0x100164cc], `CharSelectWindow_c` [0x1000ee0f] + `CharSelectItem_c` [0x1000e7f8],
  `CharActivateWindow_c` [0x1000d4e6], `CharDeleteWindow_c` [0x1000de47], `LimboWindow_c` [0x10010311],
  `LoginBrowserWindow_c` [0x10010cbc], the 3D backdrop `LoginWorld_c` [0x10015e89], the character preview
  `CharacterViewer_c` [0x10005701] and the loading screen `ServerLogin3DModule_t` [0x100175f0].

## 1. Flow (state machine)

`LoginModule_c::Show(State_e)` [GUI 0x10011570] — the states, exactly as coded:

| state | meaning | windows shown | `LoginWorld_c::SetStage` |
|---|---|---|---|
| 0 | login | `LoginWindow` (centred, `Focus()`); `LoginBrowserWindow` only if an error URL is set | 0 |
| 1 | connecting | `ProgressWindow_c(timeout = GeneralNetworkTimeout)`, default **90 s** (MainPrefs.xml) | 0 |
| 2 | limbo (character already active on the server) | `LimboWindow` | (unchanged) |
| 3 | character selection | `CharSelectWindow` (`Show(true)`); before: `ClearUserConfig(false,true)`, DValue `CharacterID`=0, `Client_t::s_nCharID`=0 | 1 |
| 4 | joining zone after "Play" | `ProgressWindow_c(timeout = DimensionNetworkTimeout)`, default **30 s** | (unchanged) |
| 5 | (`SlotInitializeCC`) nothing visible | — | — |

Before any state change the windows of the other states are hidden (login window hidden when state≠0, char select when state≠3,
limbo when ≠2, progress closed on every call).

Sequence:

1. `LoginModule_c::SlotInitialize(bool)` [0x10012204]: `ResetConnectionAndConfig`, `SandyInterfaceModule_t::StopAllSoundEffects`,
   viewport = full display, creates `LoginWorld_c(false)`, `LoginWindow_c`, `CharSelectWindow_c(Rect(0,0,W,H), loginWorld)`,
   `LimboWindow_c`, `LoginBrowserWindow_c`, enables user input, sends AFCM message (0x12, 6), then `Show(0)` (or, with the
   `bool` set, logs in directly with the stored credentials `SlotLoginAccount`).
2. Login button / Enter → signal `(username, password)` → `SlotLoginAccount` [0x10011f0c]: `ResetConnectionAndConfig`,
   `Client_t::SetPlayerName/SetPassword`, `Show(1)`, `Client_t::ConnectToLH()`; a non-zero return → `ShowError(1,0)`.
3. `LoginModule_c::SlotLoginReply(code, arg)` [0x10011f8f]: `0x0e` or `0x4e` → store `Username` DValue, `LoadUserConfig(name, 0)`;
   `0x0e` → `Show(3)` (character list; `CharSelectWindow::SlotCharListReceived` fills the list); `0x17` (character logged in) → DValue
   `CharacterID` = `Client_t::s_nCharID`, `LoadUserConfig(name, id)`, `AFCM::AddProgram(5)` (starts the loading screen, §7);
   `0x0d`, `0x10`, `0x21` → `ShowError(code,arg)`; everything else is re-emitted on a global signal (+0x184).
4. `CharSelectWindow` "Play" → `SlotSelectCharacter(id, name)` [0x100119f7]: `Client_t::s_nCharID = id; Client_t::LoginCharacter(); Show(4)`.
5. `LoginModule_c::SlotLoginLimbo(secs, CharacterInfo)` [0x10011a6c] (server says the character is already in the zone):
   timeout = max(secs, 10) → `Show(2)`.
6. Timeouts: `SlotLoginTimeout`, `SlotSelectCharacterTimeout`, `SlotLimboTimeout`, `SlotCharSelectQuitPressed` all just `Show(0)`
   (which runs `ResetConnectionAndConfig` = close connection, clear user prefs, clear name/password).
7. Quit (login window "Quit"/window-close): `AFCM::Send(10, 0x109)` [0x1001131b] = shut the client down.

## 2. The 3D backdrop shared by login and character selection — `LoginWorld_c`

**It is not a playfield** (not 896 "Character Creation Lab Dng", no statels, no sky, no terrain). `LoginWorld_c::LoginWorld_c(bool)`
[GUI 0x10015e89] builds four "stages" (0…3; only 0 and 1 are ever shown). For every stage `AddMesh(stage, name)` [0x10015dc4]
loads these five `rdb 1010001` (0xf6951) records **by name** through `InstanceManager_t::GetTypeInstance(0xf6951, name)`:

| rdb 1010001 id | name |
|---|---|
| 200350 | `charactercreation_main.abiff` |
| 200967 | `charactercreation_professions.abiff` |
| 200997 | `charactercreation_professions02.abiff` |
| 201000 | `charactercreation_nanoeffect.abiff` |
| 201002 | `charactercreation_adventurer.abiff` |

(`charactercreation_sun.abiff` 200536 and `charactercreation_seq01.abiff` 200994 exist but are not loaded here.) Each mesh instance is a
`VisualMesh_t` positioned at `(0, stage × −0.01, 0)` (the `double` at 0x101a9f90 = −0.01 – a z-fighting offset so the stages'
duplicate geometry does not overlap; `FMUL double [0x101a9f90]` in `AddMesh`). All meshes are made invisible until their stage is shown
(`SlotMeshReady` → `DisableVisibility`; `Show(stage)` → `EnableVisibility` for the stage's five meshes).

**Camera** (identical for every stage; set by `SetStage` [0x10015d6b] → `VisualCamera_t::SetPosition/SetRotation`):

| field | value | source |
|---|---|---|
| position (x,y,z) | **(0.657694, 2.27458, −10.5489)** | floats at 0x101a9a9c, 0x101a9a98, 0x101a9a94 (ctor stores them into the stage record at +0) |
| rotation quaternion (x,y,z,w) | **(0.000490287, −0.994476, 0.104805, −0.00575912)** | floats 0x101aa064, 0x101aa060, 0x101aa05c, 0x101aa058 (stage record +0xc; `Quaternion_t` default is (0,0,0,1) = `param_1[6]=1.0` in `FUN_10016273` so w is last) |
| FOV | 1.0472 rad = **60° horizontal** (vertical = 2·atan(tan 30° / aspect): 46.8° at 4:3, 39.7° at 16:10, 36.0° at 16:9) | 0x101a9f98; passed as first arg of `VisualCamera_t::VisualCamera_t(fov, aspect, near, far)`, see *Lens* below |
| aspect | `DisplayWidth / DisplayHeight` | ctor |
| near / far | **0.5 / 1000** | 0x101a9f9c / 0x101a9fa0 |

Space is AO's left-handed, Y-up, 1 unit = 1 m (`docs/formats.md` § mesh). The yaw is ≈ 2·asin(0.9945) ≈ 168° with a 12° downward pitch
(2·asin(0.1048)), i.e. the camera looks along −Z.

**Lens — the 60° is the HORIZONTAL field of view (resolved).** `VisualCamera_t::VisualCamera_t(fov, aspect, near, far)` [DisplaySystem
0x1006b12f] builds `RCamera_t(fov, aspect, near, far, parent, anim)` [randy31 0x1002a68a, args at `[EBP+8..0x14]`]: `t = tan(fov · 0.5)`
(the double at 0x1009fd98 is 0.5, `_CItan` via 0x10078c3a), view-plane window `left = −t, right = +t, top = t / aspect, bottom = −t / aspect`
(object +0xa4 / 0xac / 0xa8 / 0xb0), `near` +0xb4, `far` +0xb8. `RCamera_t::GetTransformationMatrix` [0x1002ab22] turns it into the D3D
left-handed projection `m00 = 2 / (right − left) = 1 / tan(fov/2)`, `m11 = m00 · 2 / (top − bottom) = aspect / tan(fov/2)`,
`m22 = far / (far − near)`, `m32 = −near · far / (far − near)`, `m23 = 1` (x scale independent of the aspect ⇒ horizontal angle).
`RCamera_t::SetViewPlaneWindow(fov, aspect)` [0x1002a8e1] uses the same two lines, and the character-creation module calls it
(`VisualCamera_t::SetViewPlaneWindow(degFOV · π / 180, aspect)`, GUI 0x1011c81c in `FrameProcess` 0x1011c02c): **`degFOV` of
`CharCreateCamera.dat` (§10) is horizontal as well.** `LoginWorld_c` never calls `SetViewPlaneWindow`; its aspect is `DisplayWidth /
DisplayHeight` (the window's, here). Implemented: `ao_scene::Lens { fov, horizontal, near, far }` (`Scene::lens`, `Lens::vertical_fov(aspect)`),
used by `ao-render` (`Renderer::render`), `ao_formats::screens::LOGIN_LENS` = 60° horizontal, 0.5 / 1000. The free-fly viewer and the playfields keep
`Lens::default()` (60° vertical, near 0.2, far from the fog) — whether the in-world camera is also horizontal is not examined here.

**Lighting — what the client actually does (resolved).** No code of `LoginWorld_c`, `CCCharacter_t`, `CharacterViewer_c`, `LoginModule_c` or
`ServerLogin3DModule_t` creates lights, sets an ambient colour or fog (searched every reference to `*Light*`, `*Ambient*`, `*Fog*` in GUI.dll:
only `AnarchyGround_t::ToggleLightingFix`, a pref string `FogMode`, and the character-*creation* `CCCharacter_t::ShowSelectionGlow` below). So the scene is
lit by (a) the device state that `Randy_t`'s reset leaves and (b) the lights stored in the meshes:

* **Device state** — `FUN_10041ede` [randy31 0x10041ede, run by the `Randy_t` constructor 0x10043365] sets every state explicitly
  (`render_t::SetRenderState` wrapper 0x1002397c, `SetTextureStageState` wrapper 0x10023adc): `LIGHTING` (137) = 1, **`AMBIENT` (139) = 0 (black)**,
  `COLORVERTEX` (141) = 1, `LOCALVIEWER` (142) = 1, `NORMALIZENORMALS` (143) = 0 (1 only when `Randy_t::s_eHardwareLevel & 2`), material sources
  `DIFFUSE/SPECULAR/AMBIENT` (145–147) = `COLOR1/COLOR2/COLOR2`, `EMISSIVE` (148) = `MATERIAL`, **`SPECULARENABLE` (29) = 0**, **`FOGENABLE` (28) = 0**
  (`FOGCOLOR` 0x00ff0000, `FOGTABLEMODE` LINEAR, density 0 — never enabled here: `Randy_t::EnableFog` / `SetFogParameters` are only called from the
  world's `VisualFog_t`), `ZENABLE` 1 (2 = W-buffer when `EnableWBuffer`), `ZWRITE` 1, `ZFUNC` LESSEQUAL, `CULLMODE` CCW (3), `SHADEMODE` GOURAUD, dithering only on 16-bit
  buffers, `ALPHABLEND` 0 with SRCALPHA / INVSRCALPHA, `ALPHATEST` 0. Texture stage 0: `COLOROP` = `ALPHAOP` = MODULATE (texture × diffuse), address mode WRAP;
  **filtering `MAG` = `MIN` = LINEAR, `MIP` = LINEAR (trilinear)** whenever `D3DDEVICEDESC7.dpcTriCaps.dwTextureFilterCaps` has `LINEARMIPLINEAR` (0x20; else
  linear / point-mip, else point), max anisotropy 1 (`ao-render` already samples trilinear + repeat). No `D3DRS_AMBIENT` write happens in the login: the only
  writers of the ambient state are `VisualAmbientLight_t` (`FUN_10059d82` 0x10059d82, runs the accumulated `AddAmbientLight` maximum — fed by the world's
  `EnvironmentLightObject_t`, which `FUN_1005b86b` initialises to white; no such object exists before a world is loaded) and the statel/CAT light-texture helpers.
* **Clear colour**: the 3D `RViewPort_t` is created by `FUN_10079121` [DisplaySystem 0x10079121] and its background set to **(0, 0, 0.2)** (`_DAT_10089e34` =
  0.2 stored over `viewport+0x34..0x3c`). `DisplaySystem_t` +0x34 ("clear colour buffer", set by `ToggleClearViewPort`, 0 by default) is false, so the colour buffer is
  only cleared when `Randy_t::NeedsClearFix` says so (`FUN_100793b8`); the depth buffer is cleared every frame. The backdrop covers the whole screen, so the colour is
  invisible in practice.
* **Lights of the meshes**: `charactercreation_main.abiff` (200350) contains three `RLight_t` nodes (class with a `light_info` member = a **`D3DLIGHT7`**, 0x68 bytes,
  loaded by `RLight_t::RLight_t(ObjectArchive_c*)` randy31 0x1003fe81): all three `D3DLIGHT_POINT`, range 200, falloff 1, attenuation `(0, 0.005, 0)` (intensity
  `1 / (0.005 d)`: ≥ 1 within 200 m, so it saturates), ambient component 0, diffuse = specular colour; node positions (AO space, through the frame tree) (−15.16, −36.79, −56.78)
  **diffuse black** (contributes nothing), (4.72, −122.67, 52.50) diffuse (0.710, 0.906, 0.867) mint, (79.05, 92.25, 99.66) diffuse (1.000, 0.969, 0.906) warm white. The other four
  meshes hold none. `RLight_t::Process` (0x1003ff72) appends each light of the processed frame tree to the per-frame list; `RVisual_t::CullLights` (0x1004ce96, called from
  `RTriMesh_t::Render` 0x10049761 and the CAT mesh render paths) gives **every** visual whose `grp_mask` shares a bit with the light's (default `0xffffffff` both) all of
  the lights while at most 8 exist (no range test; D3D applies `dvRange`), and only for visuals with the `enable_light` flag, which is 1 in all 41 nodes of the five meshes.
  `RVisual_t::SetMaxActiveLightCount(8)` is set at display init. The preview character (`VisualCATMesh_t` / `CCCharacter_t`) is a normal lit visual, so it gets the same lights;
  `RandyShadowlandsData_s::EnableCATLight` is never switched on here (only the world's environment object does).
* **Vertex lighting equation** (D3D7 fixed function, per vertex, then Gouraud-interpolated): `colour = saturate(emissive + ambient_mat · (device ambient 0 + light ambient 0) +
  Σ diffuse_mat · Ld · max(N·L, 0) · atten)` (the clamp is after the whole sum, emissive included), × texture, + specular, then fog; everything in the client's gamma space. Every mesh
  node of the backdrop carries the render states 145 = 146 = 147 = 0 (material source) and 148 = 1 (`COLOR1`); the vertex format (FVF 0x112) has no colour, and a colour source whose
  vertex colour is absent falls back to the material (D3D9 documentation of `AMBIENTMATERIALSOURCE`/`DIFFUSEMATERIALSOURCE`; **[INFERENCE]** that D3D7 does the same). The material
  of the draw is the viewport's `D3DMATERIAL7`, which `RViewPort_t::SetMaterial` @randy31 0x1004b199 fills with only `opac`, `emis`, `spec · shin_str` and `shin`: **diffuse and ambient stay
  white** (`SetDefaultMaterial` @0x1004b61e), so the backdrop's `diff` / `ambi` colours (the floor `grey-shit` has `diff` 0.32) tint nothing — the earlier loader used `diff` as a tint of
  untextured materials, which the client does not do (details and the CAT/sun/colour-space RE: `docs/formats.md` *Vertex lighting*). `ao-render` evaluates this per vertex exactly like D3D7.
* Implemented: `ao_formats::mesh::decode_mesh_lights` (the `RLight_t` nodes → `ao_scene::Light`, Z mirrored, colour `diffuse^2.2` like the statel lights, black lights dropped),
  `screens::login_environment()` (ambient 0, no sun, fog off, clear (0,0,0.2)), `screens::set_login_stage` (moves meshes *and* lights, `SetStage`), used by
  `login_world_scene`, `aomac play` (`show_backdrop`, `tick_preview`) and the `login_shot` example. Before this change the backdrop was drawn with the renderer's default
  environment (ambient 0.35/0.38/0.45, a warm sun, vertical 60°): the hall looked evenly white-grey. With the engine's values only the two coloured point lights
  shine (mint, warm white): walls are tinted by the light they face, surfaces turned away from both lights are black (no ambient), the view is narrower vertically
  (horizontal 60°). Checked with `login_shot` renders of both versions (`/tmp` PNGs, not committed) and the `aomac play --fake-charlist` window.
* **Specular (implemented)**: 9 of main's materials set `SPECULARENABLE` = 1 (`hull default`, `tech plated`, `grey-shit` = the floor, …), `spec` 0.9, `shin` 10–25; `ao-render` adds `Cs · Ls · (N·H)^power · atten` per vertex after the texture stage (`LOCALVIEWER` = 1, light specular = diffuse colour; see `docs/formats.md` *Vertex lighting*). With the two distant lights the highlights are weak: the `login_shot` render differs from the per-pixel one mostly by Gouraud shading on the pedestals.
* **`CCCharacter_t::ShowSelectionGlow(bool)`** [GUI 0x1011a9ae] (the "selection glow") is a `GfxVisualShield` over the character's `RCATMesh_t`; it is switched on/off only by
  `BreedScene_t::SlotBreedButton` [0x1011385b] — the character *creation* breed hover — and **never visibly renders**; the full decode is in §12 *Breed hover glow*.
  The login and character-selection screens never call it (no glow on the selection preview).

**Verified by rendering**: `cargo run --release -p aomac --example login_shot -- 1 3 out.png` draws the five meshes (stage 1) with
this camera (`LOGIN_CAMERA`, forward = rotate +Z by the quaternion = (0.012, −0.208, −0.978), then Z mirrored for the renderer) and the
preview character (§5.5) in AO space: the pod corridor of the character-creation lab is framed symmetrically, the vanishing point is the
window at the far end and the 12° pitch puts the preview character mid-frame. The quaternion is therefore (x,y,z,w) with an active
rotation of +Z.

## 3. Login screen (state 0)

### 3.1 What is on screen

* The `LoginWorld_c` stage-0 backdrop (§2) full screen. **No** `ai_loading_login.png` / `welcome_to_rubika.jpg` / logo is drawn on the
  login screen (those are the loading-screen images, §7). `gfx/loadingimage_fullscreen.jpg` is **not referenced by any client binary**
  (grepped all DLL/EXE in the client dir): dead asset.
* `LoginWindow_c` [GUI 0x10015331]: `Window::Window(Rect(), "", "", style 1, flags 0x878)` (empty title and name), child view from
  `<gui>/Views/LoginWindow.xml` (`"%sViews/LoginWindow.xml"` at 0x101a9a…, `GetGUIPath` = `cd_image/gui/Default/`), then
  `child.SetFrame(Rect(0,0,child.GetPreferredSize(true)))`, `Window::SetFrame` to that size, `Window::MoveToCenter()`.
  `MoveToCenter` [0x10154986]: `x = floor(0.5·screenW − 0.5·frameW)`, `y = floor(0.5·screenH − 0.5·frameH)`.
  Style 1 = bordered frame with icon button + close button, no title text, no pin (§9).
* `Show(0)` recentres the window. If a `LoginBrowserWindow` error page is active the two windows are stacked (offset of ±5 px added to
  each) — only used for error pages (§3.6).

### 3.2 `Views/LoginWindow.xml` (verbatim structure, from the file)

```
View
 ├ View vertical, h_alignment=LEFT, layout_borders=Rect(10,10,10,0)
 │   ├ View horizontal
 │   │   ├ TextView  username_lbl  "Username:"  min_size Point(70,-1)  layout_borders Rect(5,5,5,0)
 │   │   ├ ComboBox  username      value ""      min=max Point(139,-1) layout_borders Rect(5,5,5,0)
 │   │   └ Button    remove_btn    label "#RemoveAccount"            layout_borders Rect(5,5,5,0)
 │   └ View horizontal
 │       ├ TextView  password_lbl  "Password:"  min_size Point(70,-1)  layout_borders Rect(5,5,5,0)
 │       └ TextInputView password  value ""      min=max Point(150,-1) layout_borders Rect(5,5,5,0)
 └ View name=button_row horizontal, h_alignment=CENTER, layout_borders Rect(10,10,10,10)
     ├ Button login_btn  label "#Login"          layout_borders Rect(5,5,5,5)
     ├ Button steam_btn  label "New Account"  view_flags=256 layout_borders Rect(5,5,5,5)
     └ Button quit_btn   label "#Quit"           layout_borders Rect(5,5,5,5)
```

`Rect(a,b,c,d)` layout borders = (left, top, right, bottom) margins [GUI View layout; GuiEngine owns the layout pass]. Labels beginning
with `#` are text keys (§8): `#Login`→"Login", `#Quit`→"Quit", `#RemoveAccount`→"Remove". `view_flags=256` on `steam_btn` is the
hidden-by-default flag: the ctor looks the button up, checks the DValue `RunningSteam` (absent/false in this client) and, if not
running under Steam, `View::Show(false)` + relayouts `button_row`; if Steam were running it would bind `SlotSteamPressed` which opens
`https://register.funcom.com/mini_signup/anarchy_steam` (`ShellExecuteA`). **So the PRK client shows exactly two buttons, Login and Quit** —
"New Account" is hidden.

### 3.3 Behaviour (LoginWindow_c)

* `username` is a **ComboBox whose text field is editable** (`dynamic_cast<ComboBox_c*>`, `TextInputView_c::GetTextView` =
  embedded text view; `SetFeatureFlags` 1). Dropdown items = remembered accounts.
* **Account list persistence**: DValue archive `LauncherConfig` (slot 1 = `MainPrefs.xml` defaults merged with the user's
  `prefs/Prefs.xml`; the shipped `prefs/Prefs.xml` has `<Archive name="LauncherConfig">` with `DisplayXPos`,`DisplayYPos`,
  `SelectedAccount` (Int16, index), `NumAccounts` (Int16), `SelectedDimension` (String), `FirstLaunch` (Bool); accounts are
  `String Account0`, `Account1`, … (`"Account%d"`). Constructor [0x10015331]: reads `NumAccounts`/`SelectedAccount`, appends each
  `Account%d` as combo item (id = the name string) and pre-fills the text with the selected one. Only **usernames** are stored,
  never passwords. `SaveAccounts` [0x10014bbd] rewrites `NumAccounts`, all `Account%d`, and `SelectedAccount` = index of the current
  `Client_t::s_cPlayerName` and stores the archive via `DistributedValue_c::SetDValue("LauncherConfig")`.
* **Remove** [`SlotRemovePressed` 0x10014e00]: finds the current text as an item, `DeleteItem`, clears the username text, `SaveAccounts`.
* **Successful login**: `LoginWindow_c::SlotLoginReply(0x0e)` [0x10014ea6] adds the used username to the combo if it is not there and
  calls `SaveAccounts` (so an account is only remembered after a *successful* login).
* **Username edited** [`SlotUsernameModified` 0x10014fa5]: password text cleared, Login button disabled, username truncated to
  **39 chars** (0x27).
* **Password edited** [`SlotPasswordModified` 0x100150a1]: Login enabled **iff** username text and password text are both non-empty;
  password truncated to **67 chars** (0x43). The password field has text feature flag 2 = `TVF_PASSWORD` (masked); the ctor ORs `|0x10`
  into its renderer feature flags (`HTMLParser_c::SetFeatureFlags` @ TextRenderer +0x148).
* **Enter** in either field → `SlotEnterPressed` → `SlotLoginPressed` (no-op if either field is empty) [0x15329/0x151bb].
  `SlotLoginPressed` emits `(username, password)` (both truncated 39/67).
* **Focus** [`Focus` 0x10014ad1]: `WindowController_c::ActivateWindow`, then the **password** field gets keyboard focus
  (cursor moved to end) — even when a username is pre-filled.
* Window close / "Quit" → `SlotQuitPressed` → `LoginModule_c::SlotLoginQuitPressed` → `AFCM::Send(10,0x109)` (shutdown).
  `SlotQuitRequested` sets `*bool=false` (cancel the generic close) then does the same.

### 3.4 Window / sound / cursor

* Title/frame: style 1 (§9), empty title, centred as above.
* Sounds: `LoginModule_c`, `LoginWindow_c`, `CharSelectWindow_c`, `ProgressWindow_c` contain **no** `SandyInterfaceModule_t::PlaySample` /
  `GetSoundID` calls (I listed every `SandyInterfaceModule_t::*` call site in GUI.dll; the only ones near this flow are
  `StopAllSoundEffects` in `SlotInitialize` and `PlayStartupMusic` in `ServerLogin3DModule_t::CharacterLoggedInMessage`, §7). Generic
  button click / hover sounds come from the base `Button_c` + `SandyInterfaceModule_t::GUISoundInitialisation`
  (SandyInterface.dll) — **UNRESOLVED** here (Audio agent). **There is no login music**: music starts only after the character is
  logged in.
* Cursor: the in-game GUI pointer is drawn by the engine from Graphics.uvga gfx `GFX_GUI_POINTER_STANDARD` (+ `_WAIT`, `_IBEAM`, `_HAND`,
  `_HOR_DRAG`, `_VER_DRAG`, `_NE_SW_DRAG`, `_NW_SE_DRAG`, `_4WAY_DRAG`; `WndBorder::UpdateMousePointer` 0x101594fc chooses the resize
  pointers); there are no `.cur` files anywhere in the client.

### 3.5 Server selection

There is **none in the game client**: `Client_t::s_nLHIPAddr/s_nLHPort` come from the command line (`IA…/IP…`) given by the PRK launcher
(`docs/protocol.md` §1); the PRK launcher obtains the list from `https://site.project-rk.com/api/status` (`ao_net::client::fetch_servers`).
`AnarchyOnline.nam`/`.til` are not referenced by `AnarchyOnline.exe` or `GUI.dll`; `LoginPrefs.xml` has no server key. The stock
launcher's `LauncherConfig/SelectedDimension` is stored in the same `prefs/Prefs.xml` archive (default `""`) but only used by `Anarchy.exe`.
The new native client therefore needs a server picker **outside** the original login window (launcher-level: a pre-login chooser fed by
`fetch_servers`, or `--server` args). This is a deliberate deviation forced by the missing launcher; the login window itself must stay
as specced.

### 3.6 Errors

`LoginModule_c::ShowError(code, arg)` [0x10011deb] is **not a dialog**: it builds a URL
`<ERRORURL><code>[-<arg>].html` (`Format("%d")`, then `"-%d"` if arg≠0, then `".html"`; `ERRORURL` = key `errorurl` of
`cd_image/data/launcher/AnarchyLauncher.url`, PRK value `http://client.project-rk.com/errors/`), sets `s_cDimensionURL` and navigates
`LoginBrowserWindow_c` (a CEF/Awesomium `BrowserView_c`) to it, then `Show(0)` — the error page appears **next to** the login window
(`LoginBrowserWindow`: `Window(Rect(0,0,780,410), "", "Browser", style 1, flags 0x938)`, `SetExternalLinksEnabled(true)`, centred;
constants 780/410 at 0x101a9c70/0x101a9c74). `AnarchyLauncher.url` is parsed in the `LoginModule_c` ctor [0x10012614]: lines
`KEY=value`, `#` comments skipped, key lower-cased: `showurl`, `accounturl`, `loginurl`, `errorurl`, and `N=value` code map lines
(`1=331 …` the "special codes" 1…7 documented in the file header: 1 server not found, 2 login handler not found, 3 server lost,
4 server timeout, 5 login handler lost, 6 player not ok, 7 character not ok). `ShowError(1,0)` is used for connect failures.
**aomac** (no embedded browser): `Play::show_error` opens the identical URL with `open` and keeps the login window up. Evidence
(curl, 2026-10-05, read-only): `http://client.project-rk.com/errors/<n>.html` 301→https and returns a generic static HTML page (teal
`#0a574f` body, "Project: Rubi-Ka" header, h2 "<title by code> - 0", body "An unknown error occurred. Please contact support", Discord
footer and 4 links); the codes tried (1, 13, 14, 331) differ only in the h2 title. Content is remote and generic, so a native
rendering would add nothing the code cannot — opening the URL is the faithful reproduction. Call mapping: connect failure `(1,0)`;
type 0x0d/0x21 replies `(type, detail)`; login connection lost `(3,0)` [INFERENCE: file header "3=Server Lost", call site not located].
The `N=value` map lines of the .url (1=331 …) are parsed by the client ctor but their consumer is UNRESOLVED; not applied.

## 4. Progress dialog (state 1 and 4)

`ProgressWindow_c::ProgressWindow_c(float timeoutSeconds)` [GUI 0x100164cc]: `Window(Rect(), "", name "Progress", style 1, flags 0x938)`;
child view `Views/ProgressDialog.xml`:

```
View vertical
 ├ TextView  message      value="Please wait..." TVF_MULTILINE  layout_borders Rect(5,5,5,5)
 ├ Button    cancel_btn   label "Cancel" (literal)               layout_borders Rect(5,5,5,5)
 └ PowerBar  progress_bar bg_gfx=GFX_GUI_HOR_BAR_EMPTY full_gfx=GFX_GUI_HOR_BAR_YELLOW  layout_borders Rect(0,0,0,0)
```

The window is sized to the preferred size, `MoveToCenter()`. `SlotFrameProcess` [0x100163c1] every frame adds the frame delta to an elapsed
counter; `progress_bar` value = `elapsed/timeout` (clamped to 1); when `elapsed ≥ timeout` the window's signal fires (→ `SlotLoginTimeout` /
`SlotSelectCharacterTimeout` → `Show(0)`). The Cancel button fires the same signal immediately [0x10016346]. The window's close request
is swallowed (`SlotQuitPressed` sets `*bool=false`). The `message` text is never changed by code ("Please wait..." is the only text;
there is no per-stage status text). Timeouts: 90 s (state 1), 30 s (state 4) — MainPrefs.xml `GeneralNetworkTimeout`/`DimensionNetworkTimeout`.

## 5. Character selection (state 3)

### 5.1 Window

`CharSelectWindow_c(Rect(0,0,W,H), LoginWorld_c*)` [GUI 0x1000ee0f]: `Window(rect = whole display, "", "", style 3, flags 0xe3c)`.
Style 3 has **no frame art at all** (`WndBorder::SetBorderGfx` else-branch: no gfx, render flags 0 — §9), so the window is a transparent
full-screen overlay over the 3D backdrop. Child view `Views/CharacterSelectionWindow.xml`, `SetFrame(window bounds)`, resize mask 0xf (all
four edges anchored). Keyboard handler `SlotKeyDown` [0x1000dd15] (active only while visible and no modal child, and only for unhandled keys);
key codes are `InputConfig` ids (table at GUI 0x10262c00: name ptr, code): **ESC=0x0e → Back button**, **ENTER=0x18 → Play button**
(only if enabled), **TAB=0x0f swallowed**, **CursorUp=0x22 → previous row**, **CursorDown=0x23 → next row** (no wrap; with nothing selected, Up selects the
last row and Down the first).

### 5.2 `Views/CharacterSelectionWindow.xml` (layout)

```
View stacked
 ├ Layer 1: View horizontal, v_alignment=top
 │   ├ HLayoutSpacer                                   (pushes the panel to the right edge)
 │   └ View layout_borders Rect(15,15,15,130)
 │       └ BorderView characters_panel  min=max width 276  alpha 0.9  color DEFAULT
 │             gfx TL/TR/BL/BR/LEFT/TOP/RIGHT/BOTTOM = GFX_GUI_TAB_BORDER_*   bg = GFX_GUI_TAB_BACKGROUND
 │           └ View layout_borders Rect(3,3,3,3)
 │               ├ ScrollView (v_scrollbar_mode=auto)
 │               │   └ ScrollViewChild vertical "characters_scroll" layout_borders Rect(10,0,10,0)
 │               │       └ View "characters_view" horizontal, layout_borders Rect(10,10,10,5), min=max width 250   (item container)
 │               └ View vertical, h_alignment=center
 │                   ├ Button  create_btn  "#NewCharacter"  layout_borders Rect(10,10,10,0)
 │                   └ TextView slots_available  "0/0 slots available"  layout_borders Rect(10,3,10,15)
 └ Layer 2: View vertical
     ├ VLayoutSpacer
     ├ View layout_borders Rect(15,15,15,0), h_alignment=CENTER
     │   └ Button login_btn  "#Play"  min_size Point(70,30)
     └ View layout_borders Rect(15,15,15,15)
         └ BorderView horizontal (same TAB_BORDER_* art, alpha 0.9, color DEFAULT)
             ├ View horizontal: Button quit_btn "#Back"   layout_borders Rect(15,15,15,15)   (bottom-left)
             ├ HLayoutSpacer
             └ View horizontal: Button delete_btn "#Delete" layout_borders Rect(15,15,15,15) (bottom-right)
```

Texts: `#NewCharacter`→"New Character", `#Play`→"Play", `#Back`→"Back", `#Delete`→"Delete".
At construction [0x1000ee0f]: `login_btn`, `create_btn`, `delete_btn` start **disabled**; `characters_view` gets a `VLayoutNode` and is resized to
panel width; the `characters_scroll` child is detached from the ScrollView's layout and re-attached as ScrollView client
(`ScrollView_c::SetClient`). `CharacterViewer_c(1,3,0)` is created (Breed 1 Solitus, sex 3 female, id 0) and hidden.

### 5.3 Item rows — `Views/CharacterSelectionItem.xml`, `CharSelectItem_c`

Per character (`CharSelectItem_c(View* parent, CharacterData_t, uint activeFlags)` [0x1000e7f8]):

```
View vertical, layout_borders Rect(0,0,0,10)
 ├ View horizontal h_alignment=LEFT: TextButton name_btn  font="HUGE" text="Vhab"(placeholder)
 │       color=0x0080E9F3  hover_color=0x00FFFFFF  pressed_color=0x00EE4444  + HLayoutSpacer
 ├ summary_view horizontal layout_borders Rect(5,0,5,0):   "Level " [level] " " [breed] " " [profession]
 │       [status_left " ("] [status] [status_right ")"]  + HLayoutSpacer
 └ detailed_view vertical layout_borders Rect(5,0,5,0): two columns
       labels (LEFT):  #Lbl_Level #Lbl_Gender #Lbl_Breed #Lbl_Profession #Lbl_Location #Lbl_Status
       values (RIGHT): level gender breed profession location status      (+ HLayoutSpacer between)
```

`name_btn` is a **toggle** text button (`ButtonBase_c::SetToggleButton(true)`) whose text is the character name. The row shows
`summary_view` while unselected and `detailed_view` while selected (`CharSelectItem_c::SetSelected` [0x1000cb1a] swaps them in the row
container and sets the toggle). Field text (`SetupView` [0x1000e2ec], row data = `CharacterData_t`; wire fields = `ao_net::msg::CharacterInfo`):

* `name` = character name; `level` = `Format("%d", level)` (`DAT_101a99a4` = "%d").
* `gender` = `GetSexStr(sex)` (lower case "male"/"female"/…), **first letter upper-cased** by the view code → "Male", "Female".
* `breed` = `GetBreedStr(breed)` — display strings are hard-coded in Gamecode (`FUN_1003227a` @GC: "Solitus","Opifex","Nanomage","Atrox",…); 
  note "Atrox" (not text.mdb's "Athrox" in category 1005/604).
* `profession` = text.mdb category **2004**, id = profession (1 Soldier … 15 Shade; `LDBface::GetText(0x7d4, prof)`), or, when the
  profession is 0 or 0xff, category 506 key `NotChosenYet` → "Not chosen yet.".
* `location` = `LoginModule_c::GetPlayfieldName(pf)` + `" (%u)"` — name from `cd_image/data/launcher/pfnrmap.dat` (`id;name` per line, `#` comments,
  loaded in the `LoginModule_c` ctor), or `"PF%d"` when unknown → e.g. "Newland City (566)".
* `status`: for a character whose "activated" flag (parallel vector from the character list, bit 0 of the per-row flag word) is **clear**:
  literal text **"Inactive"** (not localised) in colour **0xEE4444**; for activated characters `status`, `status_left`, `status_right`
  (and the detailed `status_lbl`) are removed from the view.

Selection (`SlotCharClicked` [0x1000d29b]): every other row is un-selected (`SetSelected(false)`); the clicked row is selected; **Play** is
enabled; if the index is a valid character, **Delete** is enabled and the 3D viewer is updated and shown (§5.4); DValue `SelectedCharacter`
(Int16 row index) is stored (`LoginPrefs.xml` default `-1`). After the list arrives (`SlotCharListReceived` [0x1000ea33]) the stored
`SelectedCharacter` index is re-selected if it is in range. **New Character** is enabled iff `activeCount < slotCount`
(`this+0xd8 < this+0xd4`, slot count = `SlotCharListReceived` arg `param_3/4`; the server value also goes to DValue `ExpansionFlags`).
`slots_available` = text.mdb cat 10000 key `AvailableCharSlots` "%d/%d slots available" with `(slotCount − activeCount, slotCount)`.
If the list is empty `ResizeCharactersList` inserts/removes a dummy view to force a relayout.

### 5.4 Buttons

* **New Character** (`SlotCreatePressed` [0x1000ce76]) → global signal (0x204) → hands over to the character-creation module (separate
  `CharCreateModule_t` flow, `CharCreateCamera.dat` §10; out of scope here).
* **Play** (`SlotLoginPressed` [0x1000db4e]; Enter): 
  * activated character → `SlotLoginConfirmPressed` [0x1000d8ba] → signal `(charId, name)` → `LoginModule_c::SlotSelectCharacter`.
  * inactive character and no free slot (`activeCount ≥ slotCount`) → modal `DialogBox_c` named `MatchError` with one button text-key
    `MsgBox_OK`, body key `CharacterSlotsExhausted` ("There is no available character slot to activate this character."), auto-close, centred.
  * inactive character with a free slot → `CharActivateWindow_c(parent, CharacterData, active, slots)` [0x1000d4e6], centred, modal: view
    `Views/CharacterActivateWindow.xml` (320 px wide wrapped `confirmation_text` = key `ActivateCharacterWarning` formatted with the
    character name `%s`, buttons `ok_btn` "#MsgBox_Yes" (=Yes) and `cancel_btn` "#MsgBox_Cancel"); Yes → proceeds as activated.
* **Delete** (`SlotDeletePressed` [0x1000e254]) → `CharDeleteWindow_c` [0x1000de47], centred, modal; `Views/CharacterDeleteWindow.xml`:
  wrapped `confirmation_text` (key `DeleteCharacter` "Are you sure you want to delete '%s'?"), a row `charname_lbl` "#CharName"
  ("Character Name:", min 110) + `name_input` (TextInputView min 150), buttons `ok_btn` "#MsgBox_OK", `cancel_btn` "#MsgBox_Cancel".
  OK compares the typed text to the character name; mismatch → `DialogBox_c` ("MatchError", `MsgBox_OK`, text key `NamesDontMatch`
  "The name entered does not match the name of your character."); match → `SlotDeleteOkayPressed` → `LoginModule_c::SlotDeleteCharacter(id)`
  → `Client_t::DeleteCharacter()`; `SlotCharacterDeleted` reloads the list.
* **Back** (`quit_btn`, ESC) → `SlotCharSelectQuitPressed` → `Show(0)` (back to login, connection reset).

### 5.5 The 3D character preview — what is shown

On selection `CharacterViewer_c::Update(Breed_e breed, BreedSex_e sex, uint charId)` [0x100054cd] is called with
`CharacterData_t` +0x48 (breed), +0x4c (sex), +4 (id). **Only breed, sex and the id are used** — the list's `head`, `height`, `width` fields
are **not** read by the selection screen.

`Update`:
1. `CharacterViewerModule_c::GetData(charId)` [0x1000640f]: first asks the live world (`N3Msg_GetCharacterVisuals`) — never succeeds at
   login — then the **appearance cache** `prefs/CharacterViewer.xml` (`Message::LoadFromFile`; written by `SaveCache` [0x100060c0] while in
   game, entries `Character{ID,Time,MeshID,HeadID,Breed,Sex,Fatness,Side, Cloth{AlphaMode,BodyPart,EnvTextureId,Priority,TextureID}*,
   Mesh{Attractor,Flags,MeshID,TextureID}*}`). If the character has a cache entry the body mesh `MeshID`, head `HeadID`, fatness, cloth
   textures (`SetCATTexture`) and attractor meshes (weapons/armour pieces, `AddAttractorMesh`) are applied → the preview shows the worn
   equipment. The file is absent in a fresh install.
2. otherwise (**the normal first-run case**): `CCCharacter_t::SetBreed(breed, sex, 0)` [0x1011ae14] = default naked character of that
   breed/sex with head index **0**.

`CCCharacter_t` [GUI 0x1011ad5f ctor, `ChangeMesh` 0x1011ab53, `PlayAnim` 0x1011a6b0, `RunFunction` 0x1011a7d6]:

* body model = rdb **1010002** (0xf6952) named `<breed>_<sex><build>.cir`, breed names `solitus/opifex/nanomage/athrox`
  (table 0x10272434, index 1…4), sex names `male/female` (table 0x10272448: 1 unisex, 2 male, 3 female); **Atrox (4) is forced to sex 2**
  (ctor: `if (breed==4) sex=2`); build suffix `""` for Build 1 (normal; `_thin` for 0, `_fat` for 2). The viewer is constructed with
  head 0, **Build 1, Height 1** [0x10005701: `CCCharacter_t(breed, sex, 0, 1, 1, "idle-stand_01_01")`], so `solitus_female.cir` etc.
* head = entry 0 of the (breed, sex) head table that `HeadMeshData_t` (ctor `FUN_1011d368` [GUI 0x1011d368]) builds — **insertion order of a per-(sex, breed)
  `std::vector<(mesh id, ethnicity)>`, not sorted by id** (decoded in §12 *Head table*; `ao_formats::character::head_table`, used by `screens::char_select_look`
  with `ExpansionFlags` 0). Head attached as attractor mesh (`AddAttractorMesh(0, headMesh, 4, 0)`).
* skin: `VisualCATMesh_t::SetSkinData(breed, sex, race)` with the race of the head entry → naked skin textures (rdb 1010011
  `<part>_<race><sex>[_caucation|_asian|_african]_naked.png`) on layer 0 of the five materials `hands body feet arms legs`; `ChangeMesh`/`ChangeHead`
  set no cloth, so the model's own `*_default.png` is what is drawn over the skin (green keyed out → skin-coloured hands/face), exactly
  the retail "SELECT BREED" look (formats.md § Skin; `load_player` does this, all 7 combos checked against a retail screenshot).
  `Player.equipment` / the `prefs/CharacterViewer.xml` cache (formats.md § Appearance cache) is the cloth layer `CharacterViewer_c::Update`
  [0x100054cd] puts over it for existing characters.
* animation: `PlayAnim("idle-stand_01_01", loop=false)` builds the clip name `<set>_<anim>.ani` with `set = "athrox"` for Atrox else
  `"male"`/`"female"` (e.g. `male_idle-stand_01_01.ani` rdb 1010003 id 10173, `female_idle-stand_01_01.ani` 10135,
  `athrox_idle-stand_01_01.ani` 9992). It is played **once, not looped**; `RunFunction` [0x1011a7d6] each frame: when `IsAnimDone`,
  `r = rand() % 115`: if `r < 23` play the social clip `social_table[r]` (`<set>_<name>.ani` **without** `_01_01`, e.g. `male_social-angry.ani`),
  else play `idle-stand_01_01` again. Social table (23, GUI 0x101c3f40): angry, applause, blowkiss, bow, bulge, curt, fishsize, giggle,
  greet, italian, lookout, rocky, salute, scared, scratch, strong1, strong2, strong3, surprised, surrender, thinker, thumbs, wave
  (each `social-<x>`). So ≈ 20 % of idle cycles are a random emote.
* scale: `VisualCATMesh_t::SetScale(h)` with `h = 1.0` for Height 1 (0.95 / 1.05 for Height 0 / 2, constants 0x101c3fec / 0x101c3fe8; the
  viewer always uses Height 1). Selection glow (`ShowSelectionGlow`) is not used by the viewer.
* position: `CharSelectWindow` ctor [0x1000f2db…0x1000f324]: `viewer.SetPos(cameraPos + (0, −1.9, −5.0))` with `cameraPos` =
  (0.657694, 2.27458, −10.5489) → **(0.657694, 0.37458, −15.5489)**; offsets are the floats at 0x101a9aa4 (−1.9) and 0x101a9aa0 (−5.0),
  i.e. 5 m in front of and 1.9 m below the camera. `RunFunction` then adds a feet-alignment tweak to Y for breeds 1 (Solitus) and
  3 (Nanomage) only: female: Build 1 → −0.02, Build 2 → −0.07, Build 0 → +0.01; male: Build 2 → −0.05 else −0.02 (doubles at 0x101ae2e0
  = 0.02, 0x101c3fe0 = 0.07, 0x101bb7a0 = 0.01, 0x101c3fd8 = 0.05) — with Build 1: **−0.02 for Solitus and Nanomage, 0 for Opifex/Atrox**.
* rotation: `CharacterViewer_c::SetRot` is never called by `CharSelectWindow_c` → identity (the character is not turned or spun; no
  mouse rotation handler exists in these classes). Identity faces +Z, i.e. towards the −Z-looking camera.
* visibility: hidden at construction, hidden whenever the list is (re)received or a row is clicked (`Hide` then `Show` after `Update`);
  hidden on `Show(false)`.

Mapping of the server's `CharacterInfo` (`head`, `height`, `width`) — from the **creation** side (`NameScene_t::SetState(0x1006)` →
`Client_t::CreateCharacter(breed, sex, profession, headMeshId, heightPercent, build, name, side)` [GUI 0x1011f75e → IF 0x10001928]):
`head` = rdb 1010001 id of the head mesh (`CCCharacter_t::GetHeadMeshID`), `height` = size percent 90 / 100 / 110 (from
`CCSelectedHeight` 0/1/2), `width` = `CCSelectedSize` 0/1/2 (thin/normal/fat; text.mdb cat 2006 `thin, none, fat`). These are used
in-world; the selection preview ignores them (see above). `screens.rs::char_select_look` implements the preview mapping; `build_suffix` /
`creation_preview_scale` give the creation-side meaning of `width` / `height`.

## 6. Limbo window (state 2)

`LimboWindow_c` [GUI 0x10010311]: `Window(Rect(1,1,1,1), "", "", style 1, flags 0x938)`, `Views/LimboWindow.xml`: wrapped text `#LimboMessage1`
("The following character is active on the server:") — a `BorderView` with `Lbl_Name/Level/Gender/Breed/Profession` labels and values
(`SetCharacterInfo` 0x10010788 fills them like §5.3) — wrapped `#LimboMessage2` ("Click "Login" to log in with this character. To choose another
character; wait until this dialog box disappears and log in as you would normally."), a `PowerBar countdown_bar` (bg `GFX_GUI_HOR_BAR_EMPTY`,
full `GFX_GUI_HOR_BAR_YELLOW`, value counts **down** `1 − elapsed/timeout`), and `login_btn` "Login". `SetTimeout(max(server,10) s)`; at 0 → `Show(0)`;
Login → `SlotSelectCharacter` (§1).

## 7. Loading screen (character login → zone) — `ServerLogin3DModule_t`

Registered AFCM message module 0x1b / program 5. `LoginModule_c::SlotLoginReply(0x17)` calls `AFCM::AddProgram(5)` → message 200
(`InitialiseMessage` [GUI 0x1001753e]) → `SetupScreen` [0x100170d2] (plain `RenderWindow_t` sprites, no GUI window system):

* **Full-screen image** `cd_image/gui/Default/gfx/ai_loading_login.png` (1024×768 PNG) for the normal login. `SetLoadingScreen(n)` (static, default 0,
  reset to 0 by `ShutdownMessage`) selects `gfx/welcome_to_rubika.jpg` (1024×768 JPEG) when `n ∈ {1,2}`; the only caller is character
  creation (`NameScene_t::SetState(0x1008)`: `SetLoadingScreen(rand() % 3)` – after creating a new character the welcome image is used 2/3 of
  the time). If the file cannot be loaded the fallback is gui texture id 0x19 = `GFX_GUI_AO_LOGO`. `gfx/loadingimage_fullscreen.jpg` is never loaded.
  Placement: sprite size = image size, `RenderWindow_t::ScaleToResolution(1024, 768)` [0x10020b71] ⇒ scale = `(displayW/1024, displayH/768)`
  (stretched, **non-uniform** if the aspect ≠ 4:3), `UseFilter(true)` (bilinear), `ReposCenterInPercent(0.5, 0.5)` ⇒ centred on the display. Depth 100001.
* **Black full-screen sprite** (16×16 black sprite stretched to the display, depth 100000, behind the image): stays opaque while the image/text fade **in**
  (`SetAlpha(a, false)`), and fades out **together with** the image on close (`SetAlpha(a, true)`) — i.e. image over black, then everything dissolves into the game world.
* **Text** "..Anarchy Online is loading.." (`LDBface::GetText(10000, "AO_Loading")`), `FontID_e 10` = GDI TrueType **Verdana 14 px regular** (FontSystem table at GUI
  0x10272df0, record 10 = `{bitmap 0, "verdana", size 0xe, bold 0, italic 0}`; the size is multiplied by `DisplaySystem_t::GetScaleFromLowestResolution`), colour
  **0xDDDDDD**, horizontally centred (`x = (W − textWidth)/2`), top at `y = H − 50` (0x32); drawn in its own `RenderWindow_t` (depth 100002) sized to the text.
* **Fade in**: alpha 0→1 linearly over **4000 ms** (`double` 0x101aa198; `FrameProcess` [0x10016edd] branch `DAT_102628f0 != 0`; timer = `Timer_t` +0x24,
  assumed ms); then signal 0x208 and `Client_t::RedirectToServer()` (connect to the zone/chat servers); stays opaque until the zone is ready.
* **Fade out** (message 0x135 `StartClosingLoadscreenMessage` [0x10016cd8]): alpha 1→0 over **7000 ms** (0x101aa190), then `AFCM::RemoveProgram(5)`.
* Input: while the loading screen is up a full-screen `InvisibleButton_t` swallows mouse input and static input mode 7 is set.
* Message 0x26 `CharacterLoggedInMessage` [0x10016d30]: if `SoundOnOff && MusicOn` (Prefs / LoginPrefs DValues) → `SandyInterfaceModule_t::PlayStartupMusic()`
  (Audio agent: SandyInterface.dll `SandyInterface_t::PlayStartupMusic`); then `AFCM::AddProgram(6)` and `AFCM::Send(0x13, 0xe6)`.

The progress bar of `ProgressDialog.xml` is **not** shown on the loading screen: the loading screen is image + text only; the progress dialog
is only the connect (state 1) / join (state 4) timeout dialog of §4.

## 8. Prefs and text

### 8.1 Prefs (DistributedValue slots)

`ControlCenterModule_c::LoadMainConfig` [GUI 0x1006c371]: slot 0 `cd_image/Gui/Default/Variables.xml`, slot 1 `MainPrefs.xml`, slot 2 `LoginPrefs.xml`,
slot 3 `CharPrefs.xml` (defaults from the install); then `<prefs>/Prefs.xml` is merged into slot 1 (global user settings incl. the `LauncherConfig`
archive); `LoadUserConfig(name, charId)` [0x1006bacd] then merges `<prefs>/<name…>/Prefs.xml` into slot 2 (account) and slot 3 (character).
(The exact per-user sub-path concatenation in `GetLoginPrefsPath`/`GetCharPrefsPath` is **UNRESOLVED**; the client dir has only the global
`prefs/Prefs.xml` + `prefs/NewChar`.)

Defaults that matter here (`cd_image/gui/Default/`):

| key | slot/file | default |
|---|---|---|
| `GeneralNetworkTimeout` | MainPrefs | 90 (s, state 1) |
| `DimensionNetworkTimeout` | MainPrefs | 30 (s, state 4) |
| `SoundOnOff` | MainPrefs | true |
| `DisplayWidth/Height/Bits`, `DisplayFullscreen` | MainPrefs | 1024 / 768 / 32, true |
| `PlayIntro` | MainPrefs | 1 |
| `LauncherConfig{SelectedAccount=0,NumAccounts=0,SelectedDimension="",FirstLaunch=true,DisplayX/YPos=-1}` | MainPrefs / prefs/Prefs.xml | — |
| `MasterVolume` 1.0, `SoundFXOn` true, `FXVolume` 1.0, `MusicOn` true, `MusicVolume` 1.0, `BattlemusicMode` 3, `VoiceSndFx*` | LoginPrefs | as listed |
| `SelectedCharacter` | LoginPrefs | −1 |
| `ExpansionFlags` | set from the character list | 0 |

### 8.2 Text database (`cd_image/text/text.mdb`, "MMDB")

Format (decoded from the file, matches `LDBface::MapMDBfile` / `GetTextInternal` / `ElfHash` exports of `ldb.dll`):
`"MMDB"`, `u32 ncat` (52), `ncat × {u32 category, u32 byte offset}` (last category id `0xFFFFFFFF` = string pool start), then per category an
array of `(u32 key, u32 string_offset)` pairs up to the next category; strings are NUL-terminated Latin-1 in the pool. Numeric categories
use the id as key; string-key categories use **`ElfHash(key)`** (PJW hash: `h=(h<<4)+c; g=h&0xF0000000; if g {h^=g>>24}; h&=~g`). A view label `#Key` (and
`LDBface::GetText(cat, "Key")`) is the key without the `#`. Categories used by the login flow: 700 (view/button labels), 10000 (GUI texts), 506, 2004,
1005, 2006. The values used: `Login`→"Login", `Quit`→"Quit", `RemoveAccount`→"Remove", `NewCharacter`→"New Character", `Play`→"Play", `Back`→"Back",
`Delete`→"Delete", `Lbl_Level`→"Level:", `Lbl_Gender`→"Gender:", `Lbl_Breed`→"Breed:", `Lbl_Profession`→"Profession:", `Lbl_Location`→"Location:",
`Lbl_Status`→"Status:", `Lbl_Name`→"Name:", `CharName`→"Character Name:", `MsgBox_Yes`→"Yes", `MsgBox_Cancel`→"Cancel", `MsgBox_OK`→"Ok" (700) / "OK" (10000),
`LimboMessage1/2` (see §6), `AO_Loading`, `AvailableCharSlots`, `DeleteCharacter`, `ActivateCharacterWarning`, `NamesDontMatch`,
`CharacterSlotsExhausted`, `NotChosenYet`. `ao_formats::screens::TextDb` reads these. Plain literals in the XML ("Username:", "Password:", "New Account",
"Please wait...", "Cancel", "Login") are not looked up.

## 9. Window frames used by these screens

`Window(Rect, title, name, WindowStyle_e, flags)` [GUI 0x101563ff] → `WndBorder::SetStyle(style, flags)` [0x1015b205] / `SetBorderGfx` [0x1015a82d]:

| window | style | flags |
|---|---|---|
| LoginWindow | 1 | 0x878 |
| ProgressWindow, LimboWindow, LoginBrowserWindow | 1 | 0x938 |
| CharSelectWindow | 3 | 0xe3c |

The `SetGfx`/`BorderButton_c` arguments are compiled GFX ids = line numbers of `crates/ao-gui/data/gfx_ids.txt` (GuiEngine: AFCM.dll `DynamicID_t`
static table @0x10016060; 487 built-in `GFX_*` names, uvgi-only names appended). Decoded with that table:

* style 0 → no art, render flags 7; style 2 → background only; style 3 and every other style → **no art** (CharSelectWindow is fully transparent).
* **style 1 outer frame** `BorderView_c::SetGfx(tl,tr,bl,br,left,top,right,bottom,bg)` = ids `0x1bc,0x1be,0x1b7,0x1b9,0x1ba,0x1bd,0x1bb,0x1b8,0x1bf` =
  `GFX_GUI_WINDOW3_BORDER_TL, _TR, _BL, _BR, _LEFT, _TOP, _RIGHT, _BOTTOM` and background `GFX_GUI_WINDOW_BACKGROUND` (the run
  BL,BOTTOM,BR,LEFT,RIGHT,TL,TOP,TR,`WINDOW_BACKGROUND`,`WINDOW_CLOSE_X` is `0x1b7…0x1c0`; `WINDOW3_BORDER_BL2/BR2` are later additions and
  not used). View alpha = `GUIConfig_c::GetAlphaValue(layer 1)` (GUIColors.xml / MainPrefs; not looked up here).
* **style 1 inner border** (`this+0x1b4`, local colour 0x01000000, added behind the client area): ids `0x19e,0x1a0,0x199,0x19b,0x19c,0x19f,0x19d,0x19a`
  = `GFX_GUI_TAB_BORDER_TL, _TR, _BL, _BR, _LEFT, _TOP, _RIGHT, _BOTTOM` with background `0x198` = `GFX_GUI_TAB_BACKGROUND` — the same art as the
  character panel of `CharacterSelectionWindow.xml`.
* **buttons** (`CreateBorderIcons` [0x1015aba4], created only for styles 0/1/3 **and** `flags & 0x4 == 0`): icon button `BorderButton_c(0x1c5×3)` toggle
  at `BorderID 0` (left; `GFX_GUI_WINDOW_ICON_I` is only the construction placeholder, `WndBorder::SetIcon` replaces it; it opens `PopupMenu_c` +0x208 —
  whether it is drawn when the window has no icon menu is **UNRESOLVED**), close button `BorderButton_c(0x1c0,0x1c1,0x1c2)` =
  `GFX_GUI_WINDOW_CLOSE_X`, `_STATE2`, `_STATE3` at `BorderID 1` (right) → `SlotCloseButton` [0x10159705]; pin toggle `(0x1cd,0x1ce,0x1cf)` =
  `GFX_GUI_WINDOW_PIN, _STATE2, _STATE3` only when `flags & 0x800 == 0`. So: LoginWindow (0x878, style 1) has icon + close, **no pin**;
  Progress/Limbo/Browser (0x938) icon + close, no pin; CharSelectWindow (0xe3c, `0x4` set) has none. The login window's close button raises
  `SlotQuitRequested` → quit the client; the progress window's close is swallowed.
* other flag bits seen: `0x200` → border-view flag 8, `0x100` → 0x10, `0x1000` → 2 (`Window` ctor); `0x10/0x20` → not resizable per axis (`HitTest`
  [0x101593d6]). Frame metrics (border thickness, title/icon bar height) come from the gfx sizes (`UpdateBorderSizes` [0x10159f98], `Layout` [0x1015a1d9],
  `CalculatePreferredSize` [0x101596bb]) — GuiEngine owns that.

## 10. `CharCreateCamera.dat`

Plain **text** (CRLF), written/read by the character-creation camera tool [GUI loader `FUN_10116d5a` @0x10116d5a, writer `FUN_10115d75`; file name set
in `CharCreateModule_t::CharCreateModule_t` @0x1011c8a6]. It belongs to the **character creation** scenes, not to the selection screen. Grammar:

```
CameraCount: N
 repeat N:
  ID: a b c …          (hierarchical path of ints, space separated, trailing space; the path is the camera key)
  Pos: x y z
  Rot: x y z w         (quaternion; w last [INFERENCE from LoginWorld's Quaternion_t layout + unit-norm check])
  degFOV: f
TransitionCount: M
 repeat M:
  TransitionId: a b … 
  Duration: f sec
  KeyframeCount: K
   repeat K:  C: a b …   (camera id of the keyframe)
```

53 cameras (IDs `1`, `1 1`, `1 1 1` … `5 2`; FOV 38…105°) and 23 transitions (durations 0.3 … 25 s). Camera `1 1 1…` (positions around
(63,108,40) … (26,2.7,32)) is the breed-selection fly-in, `2`, `3 …`, `4 …` the profession/appearance scenes near the origin region
(x ≈ −30…28, z ≈ −59…8), `5` / `5 1` / `5 2` far away at (−1489, 4, 52) / (−1496,−140,101). `ao_formats::screens::CharCreateCameras` is the typed parser.
The login/selection camera of §2 is **not** in this file; it is hard-coded in `LoginWorld_c`. `degFOV` is the **horizontal** field of view in degrees (`VisualCamera_t::SetViewPlaneWindow(degFOV · π/180, aspect)`, §2 *Lens*).

## 11. UNRESOLVED

* Login backdrop / preview lighting is per vertex (D3D7 Gouraud) with specular, see §2 and `docs/formats.md` *Vertex lighting*.
* Whether the in-world camera (`n3Engine_t`) uses the same horizontal convention is not examined (the free-fly / playfield viewer keeps a 60° vertical lens); the login lens and lighting are resolved in §2.
* Whether the border icon button is drawn when a window has no icon menu; the `flags & 0x800` ⇒ no pin rule is read from a decompile whose assignment was lost [INFERENCE].
* Generic GUI button sounds (SandyInterface `GUISoundInitialisation`) and the exact StartupMusic cue.
* Per-user prefs sub-path (`<prefs>/<user>/…`).
* Native replacement for the web error pages (`ERRORURL`) and the pre-login server picker (launcher functionality).
* `CharacterViewer.xml` cache is documented but its use is only relevant after the first in-game session.
* `Timer_t` unit for the 4 s / 7 s fades (assumed milliseconds).
* Head-table `race` value per entry (affects naked-skin choice only for non-default heads).

## 12. Tools used (reproduce)

`/tmp/M2UI.ScreenRE/{g.sh,dec.sh,flt.sh}` run `analyzeHeadless … -process GUI.dll -postScript D.java <addr…>` (decompile) against a private copy of
`/tmp/aomac-ghidra/dsky/gui`; extra scripts `N.java` (symbol search), `S.java` (string refs), `M.java`/`P.java` (memory/pointer tables),
`T.java` (text-DB call sites). Data checks: `text.mdb` parsed with Python; names via rdb 1000010 (`ao_formats::character::names`).

## 12. Character creation (`CharCreateModule_t`, `SceneBase_t`, Breed/Appearance/Profession/Name scenes) — implementation `aomac play` (`play/create*`, `ao_formats::create`)

RE from GUI.dll (addresses in code comments): module states 0x514 → 0x44d `RunIntro` (camera transition [5 1], `SM_Sandy_CC_Opening_Music`, 10 s black fade) → 0x44e (texts MorningStar / GeosynchronousOrbit after 4 s, fade to black in the last 3 s) → [1 1 1] (Docking/ContinuingDNA texts) → scenes 0x3e9..0x3ec (Breed, Appearance, Profession, Name) → 0x4b1 exit [4 3]. World = 6 `charactercreation_*.abiff` (+ connectors `breed_0..6`, `character_0/1/3`), `CharCreateCamera.dat` played by the rig of `FUN_1011663e` (arc-length walk, ease (1−cos πp)/2, low-pass b = 0.935−min(dt,0.035)); custom transitions are the ones decoded from each scene's Next/Prev/ProfessionChanged/MoveCamera* (see `play/create/scenes.rs`). Widgets: `MakeButton` image buttons (GFX_GUI_CC_*, positions as display fractions), `MakeLabel` banners, HTML text area (title #eee, body #ccc), name `TextInputView` (label "Nickname:", colour 0xaaaaff, letters/digits only), validation `FUN_10123be2/1012382b`, `CreateCharacter(breed, sex, prof, head mesh, 90/100/110, build, name, 0)`, server error codes → texts (`NameScene_t::SetState`), 0x1e = NicknameTaken, 0x1f = NicknameInvalid. Text keys: text.mdb cat 600 by ElfHash of the key names found in the code.
Delete: `CharacterDeleteWindow` (typed name must match, else MatchError box) → `delete_character` → row removed on `CharacterDeleted`.

**Head table** (`HeadMeshData_t`, resolved from code; `ao_formats::character::head_table(store, breed, gender, expansions)`, test `character_real.rs::head_table_follows_the_clients_builder`).
`HeadMeshDataRef_t` [GUI 0x1011a54f] ref-counts one global `HeadMeshData_t`; its first creation runs `FUN_1011d368` [0x1011d368]. `FUN_100fd7c4` is **not** a data table but the
`std::map<int, T>::operator[]` instantiation, applied to two static maps (`DAT_102726d0` = female, keys 1–3; `DAT_102726cc` = male, keys 1–4 — Atrox exists only in the male map).
Each value is a `std::vector<(mesh id, ethnicity)>` (`FUN_1011ebf2` = `push_back`, element 8 bytes), so **the order is insertion order**. A second global map (`DAT_102766a8`, `FUN_1011eb45` =
`operator[]`) keeps per mesh id `{normal, hires, lores}` (`head_<…>NN.abiff`, `_hires`, `_lores`; `CCCharacter_t` only uses the normal one). For every block the name
`sprintf("head_<name>%02d.abiff", NN)` is resolved with `InstanceManager_t::GetTypeInstance(0xf6951, …)`; if it equals the id of `default_mesh.abiff` (= not in rdb 1010001) the entry is dropped. Ethnicity
(`BreedRace_e`): 1 caucasian, 2 african, 3 asian. Blocks, in this order within a vector (`NN` ranges depend on DValue `ExpansionFlags & 2` = Shadowlands: *without* / *with*):

| (sex, breed) | blocks (`NN` limit without / with Shadowlands, skipped `NN`) |
|---|---|
| female / Opifex (2) | `opifexfemale` NN < 32 / 43 except 30 (eth 1) |
| male / Opifex | `opifexmale` NN < 30 / 43 |
| female / Solitus (1) | `solitusfemale` NN < 50 / 75 except {3,4,8,9,10,13,14,22,29,30,43,46,47} (eth 1), `solitusfemale_african` NN < 8 (eth 2), `solitusfemale_asian` NN < 7 (eth 3) |
| male / Solitus | `solitusmale` NN < 51 / 255 except {3,4,10,11,12,14,28,29,31,32,34,37,41,47}, `solitusmale_african` NN < 9, `solitusmale_asian` NN < 6 / 9 |
| male / Nano (3) | `nanomale` NN < 30 / 41 |
| female / Nano | `nanofemale` NN < 30 / 43 |
| male / Atrox (4) | `athrox` NN < 30 / 41 (`CCCharacter_t` forces sex male for Atrox) |

The sex argument of the table functions is the `BreedSex_e` value: **3 = female** (static map `DAT_102726d0`), anything else male. Reading: `CCCharacter_t::MakeHeadMeshTable` [0x1011aacb] copies the vector of
(breed, sex) (`FUN_1011d26d` = first element, `FUN_1011d0bd` = element after a given mesh id, wrapping) into `CCCharacter_t+0x50`. `GetHeadCount` = size, `GetHeadIndex` = `+0xc`, `GetHeadMeshID(i)` = `table[i].mesh`
[0x1011a6a0]; `ChangeMesh`/`ChangeHead` [0x1011ab53 / 0x1011a631] clamp an index ≥ size to 0, attach `table[i].mesh` (`AddAttractorMesh(0, mesh, 4, 0)`) and set `+0x18` (race) = `table[i].eth` for `SetSkinData`.
Appearance selector: `SelectNextHead` = index−1, `SelectPrevHead` = index+1 (mod count; the names are swapped in the decompile, `play/create/scenes.rs` follows the behaviour).

* **Index vs. id.** The index is only the selector position (pref `CCSelectedHead`). On the wire a head is always the rdb 1010001 **mesh id**: `CreateCharacter.head = GetHeadMeshID(CCSelectedHead)` (`NameScene_t::SetState(0x1006)`,
  protocol.md §5a), and `CharacterViewer_c::Update` [0x100054cd] takes the stored head as a mesh id and recovers the ethnicity with `FUN_1011d29b(breed, sex, meshId)` (linear search of the vector). The preview of a character without stored
  appearance uses index 0. `screens::char_select_look` = `head_table(…, 0)[0]`; the creation code (`play/create.rs`) holds the table, index and sends `table[index].mesh`.
* **Which `ExpansionFlags`.** The table is built when the first `CCCharacter_t` exists: `LoginModule_c::SlotInitialize` [0x10012204] constructs `CharSelectWindow_c` (0x10012379), whose `CharacterViewer_c` embeds a `CCCharacter_t`
  (0x1000f278) — before `SlotCharListReceived` [0x1000ea33] stores the server's `expansions` into DValue `ExpansionFlags` (0x1000ea5a) — and the window (hence the reference count) lives for the whole login phase, including
  character creation (`SlotCreatePressed` only emits a signal). A fresh client therefore uses the *without* column (`expansions` = 0, the default of the DValue, §8 table); the live DValue is read only at the construction.
  **UNRESOLVED**: whether `LoginModule_c` is ever destroyed and rebuilt in one process (returning from the game to the login screen) — then the table would be built with the stored flags.
* Difference to the earlier inference (all caucasian `NN` ascending, then asian, then african, no limits): the real order is caucasian, **african, asian**; the `NN` limits and skip lists cut the set (e.g. Atrox 30 heads (40 with the bit; `NN` 32
  does not exist in rdb) instead of the 41 meshes in the data, i.e. without the expansion bit; Solitus male 52 entries, first ones `0,1,2,5,6,…`).

**Breed hover glow** (`CCCharacter_t::ShowSelectionGlow`, `GfxVisualShield`; resolved: it is never displayed). `ShowSelectionGlow(true)` [GUI 0x1011a9ae] allocates a 0x2c8-byte `GfxVisualShield` [DisplaySystem 0x1001ce93] over the
character's `RCATMesh_t` (`VisualCATMesh_t+0x90`; material null, flags 0, mode 0), sets colour `0xff40ff40` (vtable+0x54 stores `+0x18c`), `+0x1a8` = 0.2 (0x101ae2ec), `+0x2c0` = 1000.0 (0x101a9fa0),
`GfxVisual::EnableRendering` (adds it to `RandyRoot_t`) and `RunFunction(0)`; `ShowSelectionGlow(false)` — or `true` when a glow already exists — deletes it (vtable[0], `FUN_1001d000` unregisters the vertex callback).
What it would draw: `FUN_1001ce63` (Process) registers the vertex-process callback `FUN_1001cd09` on the CAT mesh; the callback copies every skinned batch (vertex 32 B: pos, normal, uv) into a 24-byte-vertex
buffer (FVF 0x142 = XYZ|DIFFUSE|TEX1) with `pos' = pos + normal · 0.01` (`+0x1c8` = `[0x1008ae90]`), the index list re-based, and `FUN_1001c94f` writes the vertex colour: RGB = 0x40ff40, alpha =
`ftol(255 · 0.2 · sin²(min(1000·1.0, π/2)))` = 51, i.e. a constant (0.25, 1.0, 0.25, 0.2) for every vertex (the lit/pulsing branch `+0x1ac` is never enabled). Device states (`FUN_1001c6e1`, render priority 6):
CLIPPING 1, LIGHTING 0, ZWRITE 0, COLORVERTEX 1, emissive source = vertex colour 1, CULLMODE none (mode `+0x1f0` = 0), stage 0 = vertex diffuse for colour and alpha (no texture), ALPHABLEND 1 with SRC = SRCALPHA,
DEST = INVSRCALPHA (`+0x190` = 0; ONE only for the additive flag). `FUN_1001c8a5` (vtable+0x34) draws it with the visual's world matrix (`RenderTriangleList(0x142, …)`) once `+0x178 == 0` (rendering enabled).
**Why nothing is visible**: the only callers are the two calls in `BreedScene_t::SlotBreedButton` [0x1011385b]. On a hover change the function first stores `+0x60 = +0x64` (hovered breed 1–7, 0 none), then — if the hovered breed is not the
selected `+0x68` and its character exists — calls `ShowSelectionGlow(chars[+0x64], true)` + the mouse-over sample (`PlaySample(+0x50)`, asm 0x10113a8d), and right after that `ShowSelectionGlow(chars[+0x60], false)` (asm 0x10113abd) —
the *same* character, because `+0x60` was just overwritten. The glow is created and destroyed inside one call, no frame is rendered in between; the object of a previously hovered breed is never created either. No other code calls
the function (xrefs: only these two, plus the export table; no other client DLL imports it). The port therefore reproduces the sound only (`play/create/scenes.rs`, `SM_Sandy_CC_GUI_Mouseover`); a renderer for the shield would be dead code.

**Profession info text (RE, GUI.dll `ProfessionScene_t` ctor 0x10121225, `ProfButton_t` slots, `SceneBase_t::SetNewInfoText` 0x101231d0)**: the 14 `ProfButton_t` entries (0x6c bytes, array at `this+0x60`, index `k` = profession 1..14, scene pointer at `+0x68`) hold three strings loaded with `LDBface::GetText(600, key)`: `+0x14` `Inspect<Prof>` (title when hovered), `+0x30` `<Prof>Selected` (title when selected), `+0x4c` `Description<Prof>` (body of both); the mouse-transition slot `FUN_1012096a` (connected to the button's `+0x7c` signal, `MouseTransition_e` = 2 enter / 3 leave) does: **2** → `PlaySample(scene+0x50)` (`SM_Sandy_CC_GUI_Mouseover`) + `SetNewInfoText(Inspect, Description)`; **3** → virtual `SceneBase_t::SetBackgroundText` (vtable +0x18). `ProfessionScene_t::SetBackgroundText` 0x101211b4: `prof == 0` → `ProfButtonDefault` / `ProfessionDefault` (map entry 0xf of `CharCreateModule`), else `<Prof>Selected` + `Description<Prof>` of the *selected* profession. The click slot (`0x10121219`) only calls `ProfessionChanged(k)` (0x10120f55: stores `CCSelectedProfession`, camera transition, `SM_Sandy_CC_<Prof>` sample, enables Next) and **never touches the info text**. Resulting state machine: leave/scene start → "<selected> Selected" (or the default); enter → "Click to inspect <X>" (also over the already selected profession and over *disabled* buttons, since `Window::_FindView` @0x1015472a ignores the enabled flag and `Window::_CallMouseMoved` @0x101559b3 emits the view signal before the virtual `ButtonBase_c::MouseMove` @0x10129585); click → text unchanged while the pointer stays on the button ("Click to inspect" stays until it leaves). `play/create/scenes.rs` already did all of this except the disabled-button case (Keeper/Shade without Shadowlands, Next/Back/Finish while disabled): fixed, they now give sound + text on hover. Window captures (`--fake-charlist`): Soldier hover → "Click to inspect the Soldier."; after the click the same text stays under the pointer; moving off → "Soldier Selected."; hovering the disabled Keeper → "Click to inspect the Keeper.".

**`SM_Sandy_CC_Nick` / `SM_Sandy_CC_Name` (RE + data)**: `NameScene_t::StartServerCreation` (0x1011fcac) and `NameScene_t::SetState` (0x1011f75e) cache `GetSoundID("SM_Sandy_CC_Name")` (empty name) / `GetSoundID("SM_Sandy_CC_Nick")` (invalid name, states 0x1e / 0x1f) and `PlaySample` them. `SandyInterfaceModule_t::GetSoundID` (SI 0x100071ca) is the pure hash `CreateSoundID` (case-insensitive, no prefix or alias rule); `PlaySample(id)` (SI 0x100070e3) = `GetSoundPointer(id)` (map lookup in the table `ParseSourceFile` fills, SI 0x100023ea; a miss gives 0) → `SandyInterface_t::PlaySample` returns at once for a null sound. The only banks are `sound/SourceFiles/SM_Sandy_Gui.sbf` and `SM_Sandy_Game_Dummy.sbf` (the only `.sbf` strings of SI; `anarchy.sws` is the music project). A byte scan of both banks and the `.sws` for the hashes `0xbf7701a7` (`…_Nick`) and `0xbf3771d7` (`…_Name`) finds nothing, while every other one of the 32 `SM_*` names in GUI.dll (30 `SM_Sandy_CC_*`, e.g. `…_GUI_Error` `0xe0b076ff`, `…_Name_Character` `0x59ad9955`) is present. So the original is **silent** here (a latent bug of the shipped client); the port does not call them any more (`cc_message(…, None)`), which also removes the "unknown sound" log lines.

**Remaining gaps / UNRESOLVED**: selection glow: nothing to render — the client creates and destroys it within one call (§12 *Breed hover glow*); `GetScaleFromLowestResolution` taken as H/768; receiver of global signal 0x184 (`Message`) unknown → notice box; after a created character the start playfield needs the zone messages (M3); `AOMAC_CC_SKIP_INTRO=1` debug env jumps to the breed scene.

**Camera roll**: the rig used to pass only the view direction of the `Rot` quaternion; `ao_render::Camera` now has a `roll` (0 = horizon-level, `Camera::look_at`; free-fly unchanged) and `Camera::look_to_up(eye, forward, up)`; `render()` builds the view with `look_to_rh(pos, forward, cam.up())`. The creation cameras pass the full quaternion orientation (`Pose::up()` = +Y rotated by `q`, Z-mirrored like the position). Many authored cameras roll (ids `1 1 3..6` and `1 1 8` by 40–85°, `5 1`/`5 2` 13.7°): window captures of the docking fly-in show the hangar tilting in and levelling out at the breed camera; without roll the same moment shows the hangar floor/ceiling at the wrong angles (black void below), so the authored roll is required for the fly-in.

**Esc (RE, GUI.dll)**: `CharCreateModule_t::SlotEscPressed` (0x1011b375, connected to the `GUIModule_c` signal in `InitialiseMessage`; `FlowControlModule_t::EscapePressedMessage` 0x1002899e, AFCM message 0xad, emits it; `DialogBox_c::SlotEscPressed` hears the same signal) calls camera-tool command 0x31 (`FUN_10115922`, jump table 0x10272330 → 0x1011660a): if a camera transition is playing it stops and the camera snaps to the transition's last key camera (`FUN_10116248(count)`); otherwise nothing. So Esc skips the intro fly-by [5 1], the docking fly-in [1 1 1] and the 1–4 s scene-change moves (the scene then starts at once); it does nothing inside a scene. `CameraControlMessage` (0x1011b0ff, DIK hotkeys incl. Backspace → 0x31) is never registered: dead developer code. Implemented as `CameraRig::stop`.

**Visual verification (`aomac play --fake-charlist`, fake in-process server events for random name / created / hand-off; "Taken" = name in use)**: intro (stars, rings, planet, MorningStar texts), docking fly-in with texts, Breed, Appearance (camera ends at [2 1], head hot zone over the head; head cycling, Short/Medium/Tall = 0.95/1/1.05 model scale, Slender/Medium/Heavy bodies differ), Profession (14 figures; fixed: profession meshes were not uploaded on scene entry), Name (Suggest, validation boxes for length/digits, NicknameTaken, Finish/Back disabling: Back stays disabled after an error exactly like `SetState(0)`), exit → loading screen after the 1 s boarding timer, Delete window (MatchError box now stacks over the delete window as `DialogBox_c::Go` does; correct name removes the row). Fixed: camera set from input handlers was overwritten (selection backdrop wrong after leaving creation). `SM_Sandy_CC_Nick/Name` are not in the sound bank (silent, logged). Roll of the camera quaternions is ignored (yaw/pitch renderer; real roll ≤ 3° except the fly-in 1 1 3..10 up to 40°).
