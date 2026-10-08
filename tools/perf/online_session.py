#!/usr/bin/env python3
"""Own an online measurement's server/client processes, and wait for every child."""
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time

SERVER_READY_SECONDS = 180
MODERN_CLIENT_SECONDS = 600
FAITHFUL_CLIENT_SECONDS = 400
DEFAULT_CYCLES = 400
FIXED_CLOCK = '1700000000000'


def host_load():
    return subprocess.check_output(['sysctl', '-n', 'vm.loadavg'], text=True).strip()



def usage():
    print('usage: modern_online.sh BIN OUTDIR PREFIX CYCLES WxH [PREFS_FILE] [-- extra client args]\n'
          '       online.sh BIN OUTDIR PREFIX MODE [CYCLES] [PREFS_FILE] [-- extra client args]',
          file=sys.stderr)
    return 2


def archive_session_outputs(output, prefix, server):
    suffixes = ('.log', '.png', '.trace', '.tsv', '-modern.tsv', '-prof.csv',
                '-stats.tsv', '-load.txt')
    previous = [output / f'{prefix}{suffix}' for suffix in suffixes]
    previous = [path for path in previous if path.is_file()]
    if previous:
        archive = server / 'previous'
        archive.mkdir()
        for path in previous:
            path.rename(archive / path.name)


def main():
    if len(sys.argv) < 5 or sys.argv[1] not in ('modern', 'faithful'):
        return usage()
    mode, binary, output, prefix, *rest = sys.argv[1:]
    modern = mode == 'modern'
    cycles, size, prefs = DEFAULT_CYCLES, '1024x768', None
    if modern:
        if len(rest) < 2:
            return usage()
        cycles, size, *rest = rest
    else:
        if not rest:
            return usage()
        capture, *rest = rest
        if rest and rest[0] != '--':
            cycles, *rest = rest
    if rest and rest[0] != '--':
        prefs, *rest = rest
    if rest and rest[0] == '--':
        rest = rest[1:]
    if not str(cycles).isdigit() or not re.fullmatch(r'\d+x\d+', size):
        raise ValueError('cycles must be an integer and size must be WxH')
    if not prefix or '/' in prefix:
        raise ValueError('prefix must be a plain name')
    binary = Path(binary).resolve()
    if not os.access(binary, os.X_OK):
        raise ValueError(f'not an executable: {binary}')
    output = Path(output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    repo = Path(__file__).resolve().parents[2]
    lobby_port = int(os.environ.get('PERF_PORT', '48100' if modern else '48110'))
    world_port = lobby_port + 1
    if lobby_port in (43594, 43595) or world_port in (43594, 43595):
        raise ValueError('the ordinary development ports are reserved')
    server = Path(tempfile.mkdtemp(prefix=f'srv-{prefix}-', dir=output))
    (server / 'players').mkdir()
    (server / 'client').mkdir()
    if prefs:
        shutil.copyfile(prefs, server / 'client/preferences.dat')
    env = os.environ.copy()
    env.setdefault('CLIENT910_PERF_NO_GPU_TIME', '1')
    env.update(ALTO_LOBBY_PORT=str(lobby_port), ALTO_WORLD_PORT=str(world_port),
               ALTO_PLAYER_DATA_DIR=str(server / 'players'))
    processes, logs = [], []
    interrupted = False
    interruption_signal = None

    def stop(signum, _frame):
        nonlocal interrupted, interruption_signal
        interrupted = True
        interruption_signal = signum
        for process in reversed(processes):
            if process.poll() is None:
                process.send_signal(signal.SIGTERM)

    old_handlers = {s: signal.signal(s, stop) for s in (signal.SIGINT, signal.SIGTERM)}
    try:
        for role in ('lobby', 'world'):
            log = (server / f'{role}.log').open('w')
            logs.append(log)
            processes.append(subprocess.Popen(['node', f'src/lostcity/{role}.ts'],
                                               cwd=repo / 'server', env=env,
                                               stdout=log, stderr=subprocess.STDOUT))
        (server / 'pids').write_text(' '.join(str(p.pid) for p in processes) + '\n')
        deadline = time.monotonic() + SERVER_READY_SECONDS
        while True:
            ready = all(f'listening on port {port}' in (server / f'{role}.log').read_text().lower()
                        for role, port in (('lobby', lobby_port), ('world', world_port)))
            if ready:
                break
            if interrupted or any(p.poll() is not None for p in processes):
                raise RuntimeError(f'server stopped before readiness; logs in {server}')
            if time.monotonic() >= deadline:
                raise TimeoutError(f'servers did not become ready; logs in {server}')
            time.sleep(1)
        subprocess.run([str(repo / 'ref/independence/wired-guard.sh')], cwd=repo, check=True)
        env.update(CLIENT910_WINDOW_SIZE=size.replace('x', ','),
                   CLIENT910_SCREENSHOT_CYCLE=str(cycles),
                   CLIENT910_PREFERENCES_FILE=str(server / 'client/preferences.dat'),
                   CLIENT910_VARC_FILE=str(server / 'client/client-vars.dat'),
                   CLIENT910_UID192_FILE=str(server / 'client/random.dat'))
        if modern:
            env.update(MODERN_PERF_OUT=str(output / f'{prefix}-modern.tsv'),
                       CLIENT910_PROFILE_OUT=str(output / f'{prefix}-prof.csv'),
                       CLIENT910_PERF_STATS=str(output / f'{prefix}-stats.tsv'))
            teleport = os.environ.get('PERF_TELE', '3093 3250 0')
            if not re.fullmatch(r'\d+ \d+ [0-3]', teleport):
                raise ValueError("PERF_TELE must be 'x z level'")
            commands = ['--server-command', f'tele {teleport}']
        else:
            env.pop('CLIENT910_PERF_TRACE', None)
            env.pop('CLIENT910_PERF_STATS', None)
            if capture == 'trace':
                env['CLIENT910_PERF_TRACE'] = str(output / f'{prefix}.trace')
            elif capture == 'stats':
                env['CLIENT910_PERF_STATS'] = str(output / f'{prefix}.tsv')
            elif capture != 'shot':
                raise ValueError(f'unknown capture mode {capture}')
            commands = ['--server-command', 'hintarrow tile 3224 3224',
                        '--server-command', 'gamemessage hi']
        command = [str(binary), '--renderer', 'modern' if modern else env.get('RENDERER', 'faithful-gpu'),
                   '--direct-login', '--lobby-port', str(lobby_port), '--world-port', str(world_port),
                   '--username', 'perf', '--password', 'password', '--cache-dir', str(server / 'cache'),
                   '--fixed-clock', FIXED_CLOCK, '--screenshot', str(output / f'{prefix}.png'),
                   '--server-command', 'notimeout', *commands, *rest]
        if interrupted:
            return 128 + interruption_signal
        archive_session_outputs(output, prefix, server)
        load = output / f'{prefix}-load.txt'
        load.write_text(f'start {time.ctime()} no_gpu_time={env["CLIENT910_PERF_NO_GPU_TIME"]} load={host_load()}\n')
        if interrupted:
            return 128 + interruption_signal
        log = (output / f'{prefix}.log').open('w')
        logs.append(log)
        process = subprocess.Popen(command, cwd=repo / 'tools/client910', env=env,
                                   stdout=log, stderr=subprocess.STDOUT)
        processes.append(process)
        with (server / 'pids').open('a') as stream:
            stream.write(f'client {process.pid} helper {os.getpid()}\n')
        try:
            result = process.wait(timeout=MODERN_CLIENT_SECONDS if modern else FAITHFUL_CLIENT_SECONDS)
        except subprocess.TimeoutExpired:
            process.send_signal(signal.SIGTERM)
            process.wait()
            raise TimeoutError(f'client timed out; log {output / (prefix + ".log")}')
        if interrupted:
            result = 128 + interruption_signal
        with load.open('a') as stream:
            stream.write(f'end {time.ctime()} rc={result} load={host_load()}\n')
        print(f'{prefix} rc={result}; server logs {server}')
        return result
    finally:
        for process in reversed(processes):
            if process.poll() is None:
                process.send_signal(signal.SIGTERM)
        for process in reversed(processes):
            process.wait()
        for log in logs:
            log.close()
        for sig, handler in old_handlers.items():
            signal.signal(sig, handler)


if __name__ == '__main__':
    sys.exit(main())
