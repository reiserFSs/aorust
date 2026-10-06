# Pet list (`AddPetIIR_c`, `RemovePetIIR_c`)

Server -> client messages that maintain the own pet list read by `/pet`, `/tower` (docs/chat/cmd.md) — `ao_net::n3::pet`, `Zone::pets`.

| id | class | vtable | read | write | apply |
|---|---|---|---|---|---|
| `194E4F76` | `AddPetIIR_c` | 0x10160c74 | `FUN_10071429` | `FUN_10071449` | `FUN_1007145e` |
| `58742A0F` | `RemovePetIIR_c` | 0x10161268 | `FUN_10076877` | `FUN_10076897` | `FUN_100768ac` |

Body: one `Identity` (the pet). Apply: the header dynel must exist (`FUN_10058e36`), then `FUN_10052458(pet)` (add: creates the list `dynel+0x1d8 -> +0x1c` on first use, appends the identity unless it is
already in it, calls the dynel's `vtbl[0xe8]+0x40(0x1ca)` and, when `+0x140` is set, `FUN_10012a1e`/`FUN_10011e3e`) or `FUN_100523ee(pet)` (remove); then `ClearToBePassedOn` (the message is not relayed).
`FUN_10051fa2(dynel+0x1d8)` returns the list size (0 for no list). Service towers are in this list too (`/tower` tests the same size). Not seen in the live captures (no pets); layout from the code only.
The `PetCommandIIR_c` client message is in docs/chat/cmd.md.

## Pet Info window (`pet_window`)

`ActionMenu/PetMenu.xml` opens the DValue `pet_window`; `/open Pet` uses the same name.
The native GUI builder, not an XML layout, is `PetWindow_c` `FUN_1007e3ee`, activated by
`PetWindowModule_c::SlotPetWindowActivated` `0x1007eac1`. It constructs a style-0,
flags-`0x1000` window with `Rect(200,300,600,500)`, centres it, restores `PetWindowConfig`,
and appends the localized `PetWindow` tab around `PetListView_c` (`0x1007c770`).
`The Pet Window.html` confirms that losing the last pet leaves this window open,
service towers are included, and clicking a health bar targets its pet.

`PetView_c` (`0x1007d22e`) stacks a horizontal command/health row above a nano-effect
grid. The row has 4-pixel spacers, health background `0xdb`, fill `0xdd`, name label,
then primary, Follow and Wait buttons. `0x1007ccb3` chooses Heal for MonsterData
(`455`) `96`, Attack otherwise; keys are `ButtonPetHeal`, `ButtonPetAttack`,
`ButtonPetFollow`, `ButtonPetWait`. Handlers `0x1007c919`, `0x1007c936`,
`0x1007c953`, `0x1007c970` send codes `7`, `12`, `1`, `4` respectively, addressing
exactly one identity with window/arg/tower zero. Health refresh `0x1007c9ac` divides
Health (`27`) by Life (`1`), with zero for an unknown pet or zero Life.
`0x1007cf9a` strips name control characters; `0x1007d093` uses `#808080` out of tree
and `#bbbbbb` in tree before a command-status signal. The native refresh timer is
500000 microseconds. Effect-grid size index 3 is 16 pixels; spacing is 7 and
borders 6 (`101b6e54`, `101b5234`). Effect data uses the existing `BuffIIR_c` decoder
and nano database, with expired entries removed using the nano's `TimeExist`.

`play/hud_pet.rs` implements the pet-list window, targeting, type-dependent
commands, health/name updates and timed effect icons. `WindowKind::Pet` routes the
menu and chat-window command through its existing HUD lifecycle. The shared
`Zone::stat(458)` derives NPCNumPets directly from the current list, rather than
a stale received stat, so the original menu criterion becomes reachable.
