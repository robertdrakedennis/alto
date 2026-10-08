#!/usr/bin/env python3
"""One ordinary Slayer kill, task request and purchase, using the existing scanner and pathfinder."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import time

ONE = 1
ZERO = 0
NEAR_TARGET = 1
MAX_SECONDS = 600
ACTION_SECONDS = 40
FIGHT_SECONDS = 120
SOCKET_SECONDS = 10
MAX_RECENTRE_WALKS = 4
MIDPOINT_DIVISOR = 2
POLL_SECONDS = 0.2
SHARED_SHA = 'd5377a3458af568e527852b9b87d3c529c09a168428dcd363a36c19e85d38508'
ROOT = Path(os.environ['ALTO_BOSS_RUNTIME_ROOT'])
SHARED = ROOT / 'tools/client910/fixtures/session-replay/barrows/driver.py'

def module(path, name):
    specification = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(specification)
    specification.loader.exec_module(value)
    return value

if hashlib.sha256(SHARED.read_bytes()).hexdigest() != SHARED_SHA:
    raise RuntimeError('Shared ordinary controller changed')
shared = module(SHARED, 'slayer_ordinary_helpers')

class Driver(shared.Driver):
    def __init__(self, control, args):
        self.control, self.args = control, args
        self.plan = shared.load_json(args.plan)
        self.food_rules = shared.load_json(args.food_rules)
        self.symbols, self.receipts = shared.Symbols(args.symbols), shared.Receipts(args.receipts)
        self.symbols.tables['varp'] = shared.symbol_rows(Path(args.symbols) / 'varp.sym')
        self.started = time.monotonic()
        self.deadline = self.started + args.deadline_seconds
        self.requests = self.actions = self.walk_probes = ZERO
        self.state = self.next_food_tick = None
        self.last_scans, self.food_receipts = {}, []
        self.stage = 'ready'
        self.journal = Path(args.journal).open('x', encoding='utf8')
        self.query = {
            'varps': [self.symbols.get('varp', 'slayer_task_count')],
            'varbits': [self.symbols.get('varbit', name) for name in
                ('current_life_points', 'current_prayer_points', 'legacy_combat_active')]
                + [row['activationBit'] for row in self.plan['prayers']],
            'inventories': [self.symbols.get('inv', name) for name in ('backpack', 'worn_equipment', 'coin_wallet')],
            'components': [self.symbols.component(name) for name in ('window_buttons.actions', 'backpack.slots',
                'prayer_book.prayer_buttons', 'slayer_counter.count', 'slayer_counter.target', 'slayer_rewards.broad_bolts_buy')],
            'client_varbits': [self.symbols.get('varbit', 'legacy_selected_window')]
        }
        self.log('start', {'planSha256': shared.digest(args.plan), 'sharedSha256': SHARED_SHA,
            'qualification': 'Ordinary current input only; seeded initial assignment, no live assistance'})

    def task_count(self, state):
        identity = self.symbols.get('varp', 'slayer_task_count')
        rows = [row for row in state.get('varps', []) if row['id'] == identity]
        if len(rows) != ONE:
            raise shared.Stop('Native Slayer task count is not observed')
        return rows[ZERO].get('value')

    def npc_action(self, definition, choices):
        state = self.observe()
        rows, _ = self.scan('scan_npcs', {'definition': definition, 'level': state['player']['level']})
        if not rows:
            raise shared.Stop('Actual scanner has no intended ordinary NPC')
        row = min(rows, key=lambda actor: shared.distance(actor, state['player']))
        _, operation = self.operation(row, choices)
        scan = self.last_scans['scan_npcs']['response']
        if scan['data']['map'] != state['map']:
            raise shared.Stop('Installed map changed while choosing the ordinary NPC')
        try:
            after = self.act({'kind': 'npc', **{key: row[key] for key in ('index', 'definition', 'update_serial')},
                'operation': operation}, state)
        except shared.UnacceptedNpcIntent as refusal:
            # A refused stale intent emitted no input. Keep it in the journal,
            # refresh this same actor once, and count only accepted native actions.
            self.actions -= ONE
            state = self.observe()
            if state['map'] != scan['data']['map']:
                raise shared.Stop('Installed map changed after the refused NPC intent') from refusal
            current, stamp = self.scan('scan_npcs', {'definition': definition, 'level': state['player']['level']})
            matching = [actor for actor in current if actor['index'] == row['index'] and actor['definition'] == row['definition']]
            if stamp != state['map'] or len(matching) != ONE:
                raise shared.Stop('Same intended NPC was not re-observed after the refusal') from refusal
            row = matching[ZERO]
            _, refreshed = self.operation(row, choices)
            if refreshed != operation:
                raise shared.Stop('Actual NPC operation changed after the refusal') from refusal
            self.log('unaccepted_npc_intent_refreshed', {'refusal': refusal.response, 'actor': row,
                'map': stamp, 'operation': operation, 'maximumRefreshes': ONE})
            after = self.act({'kind': 'npc', **{key: row[key] for key in ('index', 'definition', 'update_serial')},
                'operation': operation}, state)
        return row, after

    def prayers(self):
        self.open_window(self.plan['toolbar']['prayerDestination'], 'prayer_book.prayer_buttons', 'Prayer')
        for rule in self.plan['prayers']:
            state = self.observe()
            if self.bit(state, rule['activationBit']) == ONE:
                continue
            parent = self.symbols.get('component', 'prayer_book.prayer_buttons')
            rows = [row for row in self.children(self.symbols.component('prayer_book.prayer_buttons'))
                if row['target'] == {'parent': parent, 'child': rule['button']} and row.get('value', {}).get('rooted_visible')]
            if len(rows) != ONE:
                raise shared.Stop('Native end-game Prayer control is unavailable')
            row = rows[ZERO]
            wanted = shared.normal_text('Activate ' + rule['name'])
            choices = [text for text in row['value'].get('ops', []) if text and
                shared.normal_text(re.sub(r'</?col(?:=[^>]*)?>', '', text, flags=re.IGNORECASE)) == wanted]
            after = self.act(self.ui_action(row, choices), state)
            self.wait('ordinary native Prayer activation', lambda current: self.bit(current, rule['activationBit']) == ONE, after)

    def open_route_doors(self, reverse=False):
        doors = self.plan.get('navigationDoors', [])
        for door in reversed(doors) if reverse else doors:
            for _ in range(MAX_RECENTRE_WALKS):
                state = self.observe()
                area = state['map']
                if (area['base_x'] <= door['x'] < area['base_x'] + area['width'] and
                        area['base_z'] <= door['z'] < area['base_z'] + area['height']):
                    break
                before = shared.tile_key(state['player'])
                midpoint = {'level': door['level'], 'x': (state['player']['x'] + door['x']) // MIDPOINT_DIVISOR,
                    'z': (state['player']['z'] + door['z']) // MIDPOINT_DIVISOR}
                self.walk(midpoint, radius=ONE, allow_partial=True)
                if shared.tile_key(self.observe()['player']) == before:
                    raise shared.Stop('Ordinary recenter walk made no physical progress')
            else:
                raise shared.Stop('Native route door remains outside the installed build area')
            self.walk(door, radius=ONE)
            state = self.observe()
            rows, stamp = self.scan('scan_locs', {'level': door['level'], 'x': door['x'], 'z': door['z'], 'radius': ONE})
            candidates = [row for row in rows if row['definition'] == door['id'] and row['x'] == door['x'] and row['z'] == door['z']]
            if not candidates:
                continue  # An ordinary neighbouring leaf already opened the pair.
            if len(candidates) != ONE or stamp != state['map']:
                raise shared.Stop('Native route door identity or installed map differs')
            row = candidates[ZERO]
            _, operation = self.operation(row, ('Open',))
            after = self.act(self.loc_action(row, operation), state)
            def opened(current):
                if shared.distance(current['player'], door) > ONE or current['player']['route_length'] != ZERO:
                    return False
                remaining, stamp = self.scan('scan_locs', {'level': door['level'], 'x': door['x'], 'z': door['z'], 'radius': ONE})
                if stamp != current['map']:
                    raise shared.Stop('Installed map changed while waiting for the ordinary door update')
                return not any(leaf['definition'] == door['id'] and leaf['x'] == door['x'] and leaf['z'] == door['z'] for leaf in remaining)
            self.wait('ordinary door approach and observed Open update', opened, after)
            self.log('native_route_door_opened', {'door': door, 'state': self.state, 'reverse': reverse})

    def full_health_food_request(self):
        state = self.open_backpack()
        passive = self.receipts.player()
        if passive['life'] != passive['maximumLife']:
            return  # Real injury is handled by the ordinary supervised food consumer.
        item = self.plan['food']['item']
        before = self.count(self.inventory(state, 'backpack'), item)
        row = self.item_control(state, 'backpack.slots', item, ('Eat',))
        after = self.act(self.ui_action(row, ('Eat',), item), state)
        result = self.wait('native full-health food refusal', lambda current:
            current['packets_applied'] > state['packets_applied'], after)
        if self.count(self.inventory(result, 'backpack'), item) != before:
            raise shared.Stop('Ordinary full-health request consumed carried food')
        self.log('native_full_health_food_refused', {'item': item, 'beforeCount': before, 'state': result})

    def run(self):
        state = self.observe()
        self.wait('ordinary ready terrain', lambda current: current.get('ready') and current.get('terrain_present'), state['cycle'])
        self.equip()
        self.prayers()
        self.open_backpack()
        self.full_health_food_request()
        self.stage = 'fight'
        self.open_route_doors()
        self.walk(self.plan['target']['spawn'], radius=NEAR_TARGET)
        self.receipts.refresh()
        before = self.receipts.player()
        enemies = [enemy for enemy in self.receipts.last_state.get('enemies', [])
            if enemy['definition'] == self.plan['target']['npc'] and enemy['hitpoints'] == self.plan['target']['profile']['hitpoints'] and enemy['visible']]
        if not enemies:
            raise shared.Stop('No independently observed full-health ordinary target')
        offset = len(self.receipts.rows)
        row, after = self.npc_action(self.plan['target']['npc'], ('Attack',))
        full = [enemy for enemy in enemies if enemy['id'] == row['index']]
        if len(full) != ONE:
            raise shared.Stop('Selected actual actor has no independent full-health publication')
        generation = full[ZERO]['generation']
        self.wait('ordinary death and durable Slayer completion', lambda current:
            any(receipt.get('kind') == 'death' and receipt.get('target', {}).get('id') == row['index'] and
                receipt['target'].get('generation') == generation and receipt.get('source', {}).get('id') == before['pid']
                for receipt in self.receipts.rows[offset:]) and self.receipts.player()['savedSlayer'].get('assignment') is None and self.task_count(current) == ZERO,
            after, supervise_food=True, deadline=min(self.deadline, time.monotonic() + self.args.fight_seconds))
        completed = self.receipts.player()
        if completed['slayerXp'] <= before['slayerXp'] or completed['savedSlayer']['streak'] != self.plan['previousStreak'] + ONE:
            raise shared.Stop('Actual kill lacks durable XP and streak credit')
        self.log('slayer_completed', {'state': self.state, 'fullHealth': full[ZERO], 'passive': completed})
        self.stage = 'master'
        self.open_route_doors(reverse=True)
        self.walk(self.plan['master'], radius=NEAR_TARGET)
        _, after = self.npc_action(self.plan['master']['npc'], ('Get task', 'Get-task'))
        self.wait('native task request and durable assignment', lambda current:
            (self.receipts.player()['savedSlayer'].get('assignment') or {}).get('master') == self.plan['master']['code'] and
            self.task_count(current) == (self.receipts.player()['savedSlayer'].get('assignment') or {}).get('remaining'), after)
        self.log('task_assigned', {'state': self.state, 'passive': self.receipts.player()})
        _, after = self.npc_action(self.plan['master']['npc'], ('Rewards',))
        self.wait('native reward control', lambda current: self.visible(current, 'slayer_rewards.broad_bolts_buy'), after)
        state = self.observe()
        reward = self.plan['reward']
        item = reward['items'][ZERO]['item']
        beforeCount = self.count(self.inventory(state, 'backpack'), item)
        beforePoints = self.receipts.player()['slayer']['points']
        row = self.component(state, 'slayer_rewards.broad_bolts_buy')
        after = self.act(self.ui_action(row, ('Buy', 'Buy 1')), state)
        self.wait('native purchase and durable resource exchange', lambda current:
            self.count(self.inventory(current, 'backpack'), item) == beforeCount + reward['items'][ZERO]['count'] and
            self.receipts.player()['savedSlayer']['points'] == beforePoints - reward['cost'], after)
        self.log('complete', {'status': 'ordinary_slayer_pass', 'state': self.state,
            'passive': self.receipts.player(), 'actions': self.actions, 'foods': self.food_receipts})

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('plan', 'food-rules', 'symbols', 'receipts', 'journal', 'control-module', 'socket'):
        parser.add_argument('--' + name, required=True)
    parser.add_argument('--deadline-seconds', type=int, default=MAX_SECONDS)
    parser.add_argument('--action-seconds', type=int, default=ACTION_SECONDS)
    parser.add_argument('--fight-seconds', type=int, default=FIGHT_SECONDS)
    parser.add_argument('--socket-seconds', type=int, default=SOCKET_SECONDS)
    args = parser.parse_args()
    if not ZERO < args.deadline_seconds <= MAX_SECONDS:
        parser.error('Finite Slayer budget required')
    control = module(Path(args.control_module), 'slayer_normal_control').Control(args.socket, args.socket_seconds)
    driver = None
    try:
        driver = Driver(control, args)
        driver.run()
    finally:
        if driver is not None:
            driver.journal.close()
        control.close()

if __name__ == '__main__':
    main()
