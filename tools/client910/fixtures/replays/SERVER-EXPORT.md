# Dev-server replay corpora

Recorded by the TypeScript dev server, replayed by the Rust client's
production decode/apply/UI paths. Regenerate every corpus (deterministic;
a second run is byte-identical):

```sh
cd server && npm run export-fixtures            # all scenarios
cd server && npm run export-fixtures -- ../tools/client910/fixtures/replays run-toggle   # one
```

Source: `server/src/lostcity/tools/export-fixtures.ts`. Each scenario drives
the real `Player`/`World`/`ClientSocket` code with a capturing socket and the
real cache and server scripts in `server/data/pack`, and checks its own
server-side invariants before writing. These used to be jest cases writing
`/tmp/alto-wave*-research`; they are not part of `npm test`.

`frames.json` is a list of rows; each row holds the exact server frames
(`[opcode, ...payload]` including size bytes, as written to the socket) plus
the server state the client must reproduce. `*.bin` files are client request
bytes the server accepted (opcode + payload).

| Directory | Server flow | Rust consumer |
|---|---|---|
| `run-toggle` | Run orb toggles, energy 50/0/100, signed run weight | `ui_run_replay::run_toggle_and_resource_cache_replay` |
| `animation-observers` | `::anim` play/repeat/clear/delay, range loss, pid reuse, two observers | `player_animation_replay` |
| `equipment-session` | wield + remove dagger (`frames.bin` raw socket stream) | `ui_equipment_replay::equipment_session_replay` |
| `equipment-observers` | equip, walk, range loss, disconnect, pid reuse, two observers | `equipment_observer_replay::equipment_observer_replay` |
| `equipment-preview` | UI + appearance batches for the animated preview | `ui_equipment_replay::equipment_preview_renders_and_animates` (GPU) |
| `loadout-stats` / `loadout-armour` | Hero Loadout open, stats, armour/dual-wield/two-handed | `ui_equipment_replay::equipment_{stats,armour}_cache_replay` (GPU) |
| `settings-tabs` | settings window, every tab, close, reopen | `ui_settings_replay` (resize burst, all tabs, graphics) |
| `settings-navigation` | Options/Settings/tabs/close through client wire requests | `ui_settings_replay` (navigation, Controls rebinds) |
| `follow-observers` | OPPLAYER3 follow while the leader runs; menu frames for pid 1 | `equipment_observer_replay::follow_observer_replay`, `player_picking_tests::player_pick_to_follow_wire_replay` |
| `navigation-menu` | SET_MOVEACTION / SET_PLAYER_OP / SHOW_FACE_HERE + FACE_SQUARE | `ui_runtime::stat_packet_tests::navigation_packets_reach_face_here_menu_output` |
| `facing-observers` | FACE_SQUARE seen by two observers, re-entry, pid reuse | `equipment_observer_replay::navigation_observer_replay` |
| `consumable-drink` | energy/super energy doses, backpack and Hero backpack | `ui_consumable_replay::consumable_drink_replay_updates_dose_and_run_energy` |
| `consumable-settings` | "destroy empty vials" checkbox, category switch, drinks | `ui_consumable_replay` (potion preferences, sibling toggles) |
| `world-map` | login, then the minimap globe (IF_BUTTON1 1465:9): varc 674 + IF_OPENSUB 1421/1422 | `app::scenario_tests::world_map_opens_from_the_globe_paints_lumbridge_and_offers_element_ops` |
| `entity-stream` | one observer's full stream: 2 players, 2 NPCs, walk/run, VARP_SMALL/LARGE (negative), VARBIT | `entity_apply_bench::entity_apply_rejects_atomically` |
