# DValues, IndependentPrefs and the option commands

Port: `crates/aomac/src/play/dvalue.rs` (store, XML), `dvalue/indep.rs` (`IndependentPrefs_t`), `dvalue/cmd.rs` (command handlers,
expression subset), tests `dvalue/tests.rs`. Owner of the instance: `Hud.dvalues` (the HUD's window flags are DValues too).
Addresses: `[GUI]` GUI.dll, `[UT]` Utils.dll (`DistributedValue_c`, `Variant`, `ExpressionParser_c`), `[IM]` InstanceManager.dll
(`IndependentPrefs_t`), `[N3]` N3.dll. `[UT]` / `[IM]` are not in the shared Ghidra project; they were imported into a scratch project.

## 1. The two stores

| store | class | file(s) | content |
|---|---|---|---|
| DValues | `DistributedValue_c` `[UT]` | XML `Prefs.xml`: `<prefs>/Prefs.xml` (category 1 Main), `<prefs>/<account>/Prefs.xml` (2 Login), `<prefs>/<account>/Char<id>/Prefs.xml` (3 Char) | typed named variables (windows open, option panel values, chat window configs as `<Archive>`s) |
| IndependentPrefs | `IndependentPrefs_t` `[IM]` | text `<prefs>/<account>/Login.cfg` (type 0) and `<prefs>/<account>/Char<id>/Char.cfg` (type 1) | the "engine" prefs the DLLs read through `GetPrefInt/Float/String` + change callbacks (`ViewDistance`, `3rdPersonCamera`, `FogMode`, ...) |

Paths: `ControlCenterModule_c::LoadUserConfig` `[GUI 0x1006bacd]` builds `GetPrefsPath()/<account>` (+ `_mkdir`, `/Prefs.xml`) for category 2 and
`GetPrefsPath()/<account>/Char<CharacterID>` for category 3 (`Preferences_t::GetCharacterPath` 0x101245f9 = `"%s%s/%s%u"`); `LoadMainConfig`
0x1006c371 loads the category 1 `GetPrefsPath()/Prefs.xml` (the shipped `prefs/Prefs.xml`). `Preferences_t::LoadLoginPrefs` 0x1012526e =
`SetDefaultLoginPrefs` + `IndependentPrefs::Load(<account>/Login.cfg, 0)`; `LoadCharPrefs` 0x101246af = `SetDefaultCharPrefs` + `Load(<char path>/Char.cfg, 1)`
(the shipped `prefs/NewChar/Char.cfg` is the template of the latter). The port keeps the layout under `AOMAC_PREFS_DIR` (`prefs::dir()`) instead of the
client's `prefs/` (the install is read-only). `SaveUserConfig` 0x10067f57 saves category 2 / 3 and then `Preferences_t::SaveLoginPrefs` / `SaveCharPrefs`
(`IndependentPrefs::Save`); the port writes all five files whenever a value changed (`flow.rs`, `DValues::take_changed`), the original on a timer (`SlotConfigSaveTimer` 0x10068114) and at shutdown.

## 2. `DistributedValue_c` `[UT]`

* Node (`ValueNode_c`): value `Variant` +0x50, persistent flag +0x60 (`keep_default`), has-min +0x61 / has-max +0x62, min +0x68, max +0x78, category +0x88;
  global `std::map<String, ValueNode>` (0x1002e618) under an `ACE_Thread_Mutex`; a second map (0x1002e628) holds the "observed" subset.
* Load order (`LoadMainConfig`): defaults `cd_image/gui/Default/Variables.xml` (cat 0), `MainPrefs.xml` (1), `LoginPrefs.xml` (2), `CharPrefs.xml` (3) with
  `LoadConfig(path, cat, true)` = `AddVariable` (overwrite, with `min=` / `max=` / `keep_default=`), then the user files with `LoadConfig(path, cat, false)`:
  `<Value>` → `SetDValue` (variables that do not exist are ignored); `<Archive>` → the user's `Message` gets every field of the default archive it lacks,
  then `SetDValue` (`KeyBindings` is exempt from the merge). `LoadConfig` 0x10003172.
* `SaveConfig(path, cat)` 0x10003805: root `<Root>`; for each variable of the category with the persistent flag: `<Value name= value=/>` (`Variant::SaveToString`)
  or `<Archive name=…>` (`Message::DumpToXML`); written by TinyXML (shipped file: `<Value name="…" value="…" />`, quote char `'` when the value contains `"`).
* `SetDValue(name, v)` 0x25a7: unknown name → nothing; below min → min, else above max → max; stored only when the value or its type differs, then the
  per-variable observers (`DistributedValue_c` instances: `SlotXChanged`) and the global observers are signalled. `GetDValue(name, bool)` of an unknown name gives `false`.
* `Variant` type codes (`SaveToString` 0x10012ff6 / `AsString` 0x131fd): 0 void, 1 ptr, 2/3/4 int8/16/32, 5 int64, 6 bool, 7 float, 8 double, 9 string, 0xa IRect,
  0xb IPoint, 0xc Message (archive), 0x10 Rect, 0x11 Point, 0x14 Identity, 0x15 Vector3. Text form (`LoadFromString` 0x12cb9): `true`/`false`, `[-]digits[.digits]` (float when a `.`),
  `"string"` (the last character is dropped unconditionally), `Rect(%f,%f,%f,%f)`, `Point(..)`, `Identity(%u,%u)`, `Vector3(..)`, `IRect`, `IPoint`, `Ptr(%p)`, `void`.
  `AsString`: `%d`, `%f` (6 decimals), `true`/`false`, the raw string, `[Archive]`.
* Names are looked up case-sensitively (`std::map<String>`; [INFERENCE] — `String::operator<` was not decompiled).

## 3. `IndependentPrefs_t` `[IM]`

* Layout: six `std::map<std::string, …>` at +0x54 login int, +0x58 login float, +0x5c login string, +0x60 char int, +0x64 char float, +0x68 char string; entry =
  value +0x28, min +0x2c, max +0x30, callback list +0x34 (`SetIntChangedCallback` / `SetFloatChangedCallback` 0x1000268e / 0x10002771; N3 registers
  `ViewDistance` with fire-now, `MouseLookInverted`, `3rdPersonCamera`, `UseNoBobCamera`). File paths at +0x00 / +0x1c / +0x38.
* `SetPrefInt/Float` (0x10002e5c / 0x10002f4f): an unregistered name is registered first with the range ±`FLT_MAX` / `INT` limits (0x10008264), then clamped to
  `[min, max]`, stored, callbacks run. `InitDefaultInt/Float` (0x10002bb5 / 0x10002d04) register `(name, default, min, max, type)`; on an existing entry they replace
  min / max and re-set the value to the default.
* File (`Save` 0x10001d6a / `Load` 0x100034bf): CRLF lines `name %d`, `name %f`, `name "%s"` — ints, floats, strings; each map sorted by name (the shipped
  `NewChar/Char.cfg` shows exactly that order); `username` / `password` are skipped in both directions; load: `%399s %399s`, first char `/` `#` or blank skips, value starting with `"`
  → string up to the next `"`, value containing `.` → float, else `sscanf %i`.
* `GetPrefEasy` 0x10002368 / `SetPrefEasy` 0x10003076 (used by `/option`): search login int, login float, login string, char int, char float, char string;
  texts `Login-pref <%s> is <%d|%f|%s>`, `Char-pref <…>`, `Changed login-pref <%s> to <…>`, `Changed char-pref <…>`. Set parses the text with `atol` **and** `atof` and uses the registered type.
  Format arguments of the int / string "Changed" messages were dropped by the decompile: the port prints the parsed value ([INFERENCE]).
* Defaults: `SetDefaultLoginPrefs` `[GUI 0x10124b33]` (operands read from the PUSH sequence, floats from 0x101a959c…): `WasCharacterCreated 1 (0..1)`, `CCSelectedBreed 0 (0..7)`, `CCSelectedHeight/Size 0 (0..3)`,
  `CCSelectedHead 0 (0..300)`, `CCSelectedProfession 0 (0..14)`, string `CCSelectedName ""`, `PreferredCamPosX/Y/Z 0 / 0.316 / −0.948 (±200)`, `PreferredCamDist 5 (0..347)`, `PreferredCameraMode 3 (0..3)`,
  `UseNoBobCamera 0`, `AspectRation 1.3333 (0..999999)`, `IsChatHidden 0`, 24 more 0/1 switches (all 1 except `MouseLookInverted` 0), `FogMode 3 (0..3)`, `GroundRendering 3 (0..3)`,
  **`ViewDistance 0.8 (0..1)`**; then one `InitDefaultInt(key, default, 0, 999999999)` per `InputConfig_t` key binding (not ported). `SetDefaultCharPrefs` `[GUI 0x1012447a]`: `IsFirstTime 1`,
  `ShowDropItemDialog 1`, `IsOrgNameShownOverHead 1`, `IsSpaceShipsShown 1`, `BuildMenuX/Y 0 (0..9999)`, `BuildMenuCX/CY 0 (0..1000)`, `IsFactionTitleShown 1`, `WaitForVertSync 0`, `MouseLagFix 0`,
  `UseOffscreenSurfaceTechnology 0`, `FadeCharacter 1`, floats `FadeCharacterStartDist 1.5 (0.1..10)`, `FadeCharacterEndDist 0.7 (0.1..10)`, `FadeCharacterEndAlpha 0.15 (0..1)`.

## 4. The commands `[GUI]`

All print through `FUN_1009b37f(text, colour)`, colour 0x51 = `CCChatCmdFeedbackError`, 0x52 = `CCChatCmdFeedbackInfo`; the literals are HTML (`&lt;` `&gt;`), values go
through `String::Escape(v, "<>&")`. `FUN_100b5adc` (`/option`, `/setoption`; `String::CompareNoCase(argv0, "/option")` = silent flag), `FUN_100b7127` (`/dvalue`).

| form | behaviour |
|---|---|
| `/option` (no name) | `Invalid syntax: <argv0> . Use /option <OptionName> [value]` (0x51; `/dvalue`: `… Use /dvalue <OptionName> [value]`) |
| `/option name` | `GetPrefEasy` hit → its text, 0x52; else DValue exists → `Variable <name> is <value>` (0x52); else `Can't find option <name>.` (0x51) |
| `/option name expr` | `SetPrefEasy` hit → nothing (`/setoption`: its `Changed … -pref` text, 0x52); else missing DValue → `Can't find option <name>.`; else `ParseExpression(argv2)`; failure → `Failed to parse expression <expr>.`; success → clamp to `GetMinMaxValues` (void bound = none), `SetDValue`, `/setoption` only: `Changed variable <name> from <old> to <new>` |
| `/dvalue name expr` | same, but a missing variable is created with `AddVariable(name, v, false, false)` (temporary, never saved), old text `none` |
| `/chardist n` | `SetDValue("DisplayCharViewDistance", Variant(atol(n)))` (`FUN_100b62ed`), fewer than 2 tokens: `Error: To few arguments` (0x51) |
| `/viewdist f` | `IndependentPrefs::SetPrefFloat("ViewDistance", (float)atof(f) * (float)0.01, 0, true)` (`FUN_100b63b8`; the double at 0x101bb7a0 is 0x3f847ae140000000 ≈ 0.01) |
| `/char&viewdist n f` | both, in that order (`FUN_100b6465`), fewer than 3 tokens: `Error: To few arguments` |

`ParseExpression` `[UT 0x10016f1d]` is a yacc-generated LALR parser (tables at 0x1001fa08 / 0x1001fb2c.. / 0x1001fc48) over `Variant` operators (`+ - * / &` …) with a context list;
it was **not** ported table by table. `dvalue/cmd.rs::parse_expr` recreates the subset the GUI's own files use: decimal/hex integers, floats, `"strings"`, `true`/`false`, `( ) ! -`,
`* / + - < > <= >= == != & | && ||`; int op int stays int, a float operand gives a float. Unknown identifiers (original: constants and `dvalue:` / `stat:` contexts) fail to parse. **GUESS**: the lexer's handling of bare words.

## 5. What the two view distances do

* `ViewDistance` (pref, 0..1, shown as % by the option panel `OptionPanel/Root.xml` `OptionSlider ViewDistance`, `value_scale 100`): `FUN_1001fc91` `[N3]`, registered at camera construction with
  fire-now (`n3Camera_t` ctor 0x10021a76), sets the camera's far plane to `max(pref × 1000, near + 50)` and calls `VisualFog_t::AddClipPlanes(near, far)`; the statel zone manager's
  view length is `GetLengthOfViewcone` = far − near. Port: `DValues::view_distance()` → `Player::set_view_distance` → the lens' `far = camera::far_plane(vd)` → `Renderer::set_lens`
  also sets `FogModel::far` (fog end) and the statel LOD `view_length`. The shipped playfield constants (`environment::VIEW_DISTANCE` 800, `LOD_VIEW_LENGTH`) equal the default 0.8 and are the loaders' initial values.
* `DisplayCharViewDistance` (DValue, Login category, default 80, min 5, max 80; option panel slider `CharactViewDistanc`, "%.0f m"): the camera owns a `DistributedValue_c` of it (n3Camera_t +0xb0, ctor 0x10021a76);
  `FUN_1001f964` `[N3]` (connected to its change signal at +0xb8 and called once in the ctor) stores `(float)v` at camera +0x174, **70** when `v < 5 || v > 80`. `n3VisualDynel_t::Run`
  `[N3 0x100196bd]`: on a ground playfield, a dynel other than the controlled one whose cap flag (+0xca) is set is hidden (`VisualCATMesh_t::DisableVisibility`) when the squared distance between the two
  dynels' global positions exceeds `cam+0x174²`. Dungeons use the room visibility instead. Port: `Dynels::char_view_distance` (set from `DValues::char_view_distance()` each frame) replaces the former
  250 m constant for characters; props keep `DRAW_DISTANCE` (guess).
* **Unresolved (GUESS)**: which dynels have the cap flag. `n3VisualDynel_t::UseCharDistCap(bool)` (N3 0x7562, ordinal 878) is exported but no DLL imports it; the only writer found is Gamecode `FUN_10058415`
  (called through a vtable at 0x1015fb34: `ClearFlags(mask)`, sets +0xca = 1 when bit 0x10000000 is cleared). Searched: `UseCharDistCap` by name/ordinal in every DLL/EXE, `+ 0xca` in Gamecode / N3 decompiles. The port caps every character.
  The distance is measured from the camera (the original: from the controlled dynel); the camera orbits within a few metres.

## 6. Not ported

`InputConfig_t` key-binding int prefs of `SetDefaultLoginPrefs`; `ExpressionParser_c` contexts (`dvalue:` / `stat:` inside `/setoption` values); the `DistributedValue_c` observer lists beyond
`DValues::take_changed` / `IndepPrefs::take_changed` polling; `ForgetAll` / `ResetToDefault` / `DeleteVariable` (no caller in the chat commands); `prefs/NewChar` consumer (docs/gui.md §10).

## Options window cross-reference
The options window (docs/gui.md "Options window") writes these DValues / IndependentPrefs live through `DValues::set` / `prefs.set_int|float`; the flow's `take_changed` saves them. `IndepPrefs::int_range/float_range` expose the registered ranges (`GetInt/FloatMinMaxValue`).
