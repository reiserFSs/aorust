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
