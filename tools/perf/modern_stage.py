#!/usr/bin/env python3
"""Stage an instrumented COPY of the tree for the modern renderer's
measurements: the renderer crate itself is never edited.

usage: modern_stage.py SOURCE DEST
  SOURCE  a worktree (its tools/ is copied; server/data/pack is linked)
  DEST    the build copy (created; must not be a git checkout)

In DEST's `rs910-render-modern`:
- `tools/perf/modern_perf_hook.rs` becomes `rs910_render_modern::modern_perf_hook`;
- every wgpu call of the non-test sources is rewritten to its counting
  `nx_*` twin (by method name; `set_vertex_buffer(s, X.slice(R))` becomes
  `nx_set_vertex_buffer(s, &X, R)` so the hook sees the buffer);
- every pass descriptor's `timestamp_writes: None` becomes the hook's
  begin/end timestamp writes, named by the pass label (or file:line);
- `ModernRenderer::draw`/`encode` get CPU phase marks (`phase`), the frame is
  opened with `frame_begin` and resolved with `frame_end`;
- `tools/perf/modern_perf_bench.rs` is added as the test module
  `frame::perf_bench` (the headless matrix bench).

It exits non-zero when an anchor is missing, so a renamed call site is
noticed instead of silently untimed. Then build with modern_bench.sh.
"""
import re
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
src = Path(sys.argv[1]).resolve()
dest = Path(sys.argv[2]).resolve()
if (dest / '.git').exists():
    sys.exit(f'refusing to stage into a git checkout: {dest}')
dest.mkdir(parents=True, exist_ok=True)
(dest / '.perf-copy').touch()
# Content-compared copy: unchanged files keep their mtimes (incremental builds).
stage = dest.parent / (dest.name + '.stage')
stage.mkdir(parents=True, exist_ok=True)
subprocess.run(['rsync', '-a', '--delete', '--exclude', 'target', '--exclude', '.git',
                f'{src}/tools/', f'{stage}/tools/'], check=True)
crate = stage / 'tools/client910/crates/rs910-render-modern'
srcdir = crate / 'src'

shutil.copy(HERE / 'modern_perf_hook.rs', srcdir / 'modern_perf_hook.rs')
lib = srcdir / 'lib.rs'
t = lib.read_text()
if 'pub mod modern_perf_hook;' not in t:
    t = re.sub(r'(\npub mod frame;)', r'\npub mod modern_perf_hook;\1', t, count=1)
    assert 'pub mod modern_perf_hook;' in t, 'lib.rs anchor: pub mod frame;'
    lib.write_text(t)

SIMPLE = [
    'create_shader_module', 'create_render_pipeline', 'create_compute_pipeline',
    'create_buffer_init', 'create_buffer', 'create_texture_with_data', 'create_texture',
    'create_bind_group', 'create_sampler', 'write_buffer', 'write_texture',
    'begin_render_pass', 'begin_compute_pass', 'copy_texture_to_texture',
    'copy_buffer_to_buffer', 'set_pipeline', 'set_bind_group', 'draw_indexed',
    'dispatch_workgroups', 'multi_draw_indexed_indirect',
]
USE = ('\n#[allow(unused_imports)]\nuse crate::modern_perf_hook::{NxComputePass as _, NxDevice as _, '
       'NxEncoder as _, NxQueue as _, NxRenderPass as _};\n')


def split_args(text, start):
    depth, args, cur, i = 0, [], [], start
    while True:
        c = text[i]
        if c in '([{':
            depth += 1
            if depth > 1:
                cur.append(c)
        elif c in ')]}':
            depth -= 1
            if depth == 0:
                args.append(''.join(cur).strip())
                return args, i + 1
            cur.append(c)
        elif c == ',' and depth == 1:
            args.append(''.join(cur).strip())
            cur = []
        else:
            cur.append(c)
        i += 1


def rewrite_buffers(s):
    for name in ('set_vertex_buffer', 'set_index_buffer'):
        out, pos = [], 0
        for m in re.finditer(r'\.(\s*)' + name + r'\(', s):
            if m.start() < pos:
                continue
            args, end = split_args(s, m.end() - 1)
            arg = args[1] if name == 'set_vertex_buffer' else args[0]
            sl = re.fullmatch(r'(?s)(.*)\.slice\((.*)\)', arg)
            if sl and name == 'set_vertex_buffer':
                new = f'.nx_{name}({args[0]}, &{sl.group(1)}, {sl.group(2)})'
            elif sl:
                new = f'.nx_{name}(&{sl.group(1)}, {sl.group(2)}, {args[1]})'
            else:
                new = f'.nx_{name}_slice({", ".join(args)})'
            out.append(s[pos:m.start()])
            out.append(new)
            pos = end
        out.append(s[pos:])
        s = ''.join(out)
    return s


def add_timestamps(s, rel):
    out, pos, n = [], 0, 0
    for m in re.finditer(r'timestamp_writes: None', s):
        head = s[:m.start()]
        r = head.rfind('RenderPassDescriptor')
        c = head.rfind('ComputePassDescriptor')
        kind = 'render' if r > c else 'compute'
        start = max(r, c)
        lab = re.search(r'label: Some\(("[^"]*"|[a-z_][a-z_0-9]*)\)', s[start:m.start()])
        # A declared pass (`frame::passes`): named by its declaration.
        declared = re.search(r'label: Some\(self\.begin_pass\(([^()]*)\)\)', s[start:m.start()])
        line = head.count('\n') + 1
        if declared:
            name = f'{declared.group(1)}.label()'
        elif lab and lab.group(1).startswith('"'):
            name = lab.group(1)
        elif lab and re.fullmatch(r'[a-z_][a-z_0-9]*', lab.group(1)):
            name = lab.group(1)  # a `&str` label variable (interned by the hook)
        else:
            name = f'"{rel}:{line}"'
        out.append(s[pos:m.start()])
        out.append(f'timestamp_writes: crate::modern_perf_hook::ts_{kind}({name})')
        pos = m.end()
        n += 1
    out.append(s[pos:])
    return ''.join(out), n


total, stamped = 0, 0
for f in sorted(srcdir.rglob('*.rs')):
    rel = f.relative_to(srcdir).as_posix()
    if f.name == 'modern_perf_hook.rs' or 'test' in f.name or '/tests' in rel:
        continue
    s = f.read_text()
    if 'modern_perf_hook::{' in s:
        continue
    orig = s
    for name in SIMPLE:
        s = re.sub(r'\.(\s*)' + name + r'\(', r'.\1nx_' + name + '(', s)
    s = re.sub(r'\bpass(\s*)\.(\s*)draw\(', r'pass\1.\2nx_draw(', s)
    s = rewrite_buffers(s)
    s, k = add_timestamps(s, rel)
    stamped += k
    if s != orig:
        s += USE
        f.write_text(s)
        total += 1
print(f'modern_stage: rewrote {total} files, {stamped} timed pass descriptors', file=sys.stderr)

# CPU phases in prepare/record and encode(): `phase(name)` before each anchor line.
DRAW = [
    ('self.ensure_targets(device, size);', None),
    ('self.check_scene(snapshot);', 'scene check'),
    ('let now = self.frame_millis();', 'environment + look'),
    ('let ssao = self.prepare_post(', 'prepare post'),
    ('let frame_uniforms = self.atmosphere_uniforms(', 'atmosphere uniforms'),
    ('self.prepare_shadows(device, queue, snapshot', 'prepare shadows'),
    ('self.prepare_lights(device, queue, snapshot, origin);', 'prepare lights'),
    ('self.prepare_grading(queue, snapshot);', 'prepare grading'),
    ('let sky_layers = self.prepare_sky(', 'prepare sky'),
    ('let list = DrawList::build(snapshot);', 'draw list build'),
    ('self.prepare_terrain(device, queue, snapshot, &list, origin);', 'prepare terrain'),
    ('self.prepare_far(device, queue, snapshot, &list, origin);', 'prepare far'),
    ('self.pose_models(snapshot, &list);', 'pose models'),
    ('let mut stats = Stats {', 'opaque entities'),
    ('self.prepare_far_locs(device, queue, snapshot, origin, None);', 'far locs (opaque)'),
    ('let opaque_draws = self.draws.len();', 'floors'),
    ('let group0_end = self.draws.len();', 'transparent entities + far locs'),
    ('self.prepare_sprites(', 'prepare sprites'),
    ('let hidden = self.roof_hidden(snapshot);', 'caster gather (roof hidden, off-screen)'),
    ('self.prepare_interior(device, queue, snapshot, hidden.as_ref());', 'prepare interior'),
    ('stats.billboards = self.stats.billboards;', 'stats'),
    ('self.prepare_probes(&prep, &uniforms);', 'prepare probes'),
    ('self.cascade_masks.clear();', 'cascade masks'),
    ('self.arena.upload(device, queue);', 'uploads (arena, instances, sky)'),
    ('if self.frame == 1 || self.frame.is_multiple_of(600) {', 'stats log'),
    ('self.prepare_point_shadows(device, queue, origin);', 'prepare point shadows'),
    ('self.prepare_water(&prep', 'prepare water'),
    ('self.prepare_caustics(device, queue, origin);', 'prepare caustics'),
    ('self.prepare_atmos(device, queue, &uniforms', 'prepare atmos'),
    ('Some(PreparedFrame {', 'handoff: acquire + UI under'),
    ('self.encode(device, encoder, view, prepared.rect, prepared.clip', 'encode: setup'),
]
ENCODE = [
    ('let recorded = self.jobs.map(', 'encode: units'),
    ('buffers.into_iter().map(|(_, b)| b).collect()', 'encode: finish'),
]
# Each encode unit's CPU time on its thread (`frame::units`).
UNIT = 'self.encode_unit(unit, &mut encoder, &cx);'
# Gauges: the loc mesh cache's size each frame, the probe captures started.
GAUGES = [
    (srcdir / 'frame/mod.rs', 'stats.statics = ',
     'crate::modern_perf_hook::count(crate::modern_perf_hook::K::MeshCache, stats.statics as u64);'),
    (srcdir / 'frame/gpu/probes.rs', 'self.probes.stats.captures += 1;',
     'crate::modern_perf_hook::count(crate::modern_perf_hook::K::ProbeCaptures, 1);'),
]
# State ownership is free to change; keep the same instrumentation when
# comparing a tree from before the split with one using explicit owners.
state_file = srcdir / 'frame/state.rs'
owners = {}
if state_file.exists():
    state = state_file.read_text()
    for owner, type_name in [
        ('device_resources', 'DeviceResources'), ('scene_resources', 'SceneResources'),
        ('frame_resources', 'FrameResources'), ('history', 'FrameHistory'),
        ('preparation', 'PreparationState'),
    ]:
        section = state.split('struct ' + type_name + ' {', 1)[1].split('\n}', 1)[0]
        for field in re.findall(r'pub\(crate\) (\w+):', section):
            owners[field] = owner

def owned_access(text, receiver):
    for field, owner in owners.items():
        text = re.sub(r'\b' + receiver + r'\.' + field + r'\b',
                      receiver + '.' + owner + '.' + field, text)
    return text

DRAW = [(owned_access(anchor, 'self'), name) for anchor, name in DRAW]
GAUGES = [(path, owned_access(anchor, 'self'), call) for path, anchor, call in GAUGES]
for path, anchor, call in GAUGES:
    t = path.read_text()
    if call in t:
        continue
    lines = t.split('\n')
    at = [i for i, l in enumerate(lines) if l.strip().startswith(anchor)]
    if len(at) != 1:
        sys.exit(f'modern_stage: gauge anchor {anchor!r} found {len(at)} times in {path.name}')
    i = at[0]
    ind = lines[i][:len(lines[i]) - len(lines[i].lstrip())]
    # After the statement (it may span lines: up to the line ending in ';').
    j = i
    while not lines[j].rstrip().endswith(';'):
        j += 1
    lines.insert(j + 1, ind + call)
    path.write_text('\n'.join(lines))

# Lane P5: `MODERN_BENCH_COLD=1` makes every module's text unique to the
# process (a cold Metal shader cache). (The render thread is the one that
# draws, `frame_begin`; the client may create the renderer on another.)
PATCHES = [
    (srcdir / 'shaders/mod.rs', 'source: wgpu::ShaderSource::Wgsl(module.source().into()),',
     'source: wgpu::ShaderSource::Wgsl(crate::modern_perf_hook::cold_nonce(module.source()).into()),'),
]
for path, anchor, repl in PATCHES:
    t = path.read_text()
    if repl in t:
        continue
    if t.count(anchor) != 1:
        sys.exit(f'modern_stage: patch anchor {anchor!r} found {t.count(anchor)} times in {path.name}')
    path.write_text(t.replace(anchor, repl))

rend = srcdir / 'frame/mod.rs'
t = rend.read_text()
if 'modern_perf_hook::frame_begin' not in t:
    lines = t.split('\n')
    draw_at = next(i for i, l in enumerate(lines) if l.strip().startswith('pub fn prepare_frame('))
    enc_at = next(i for i, l in enumerate(lines) if l.strip() in ('fn encode(', 'pub(crate) fn encode('))
    out = []
    todo_d, todo_e = list(DRAW), list(ENCODE)
    units = 0
    for i, l in enumerate(lines):
        s = l.strip()
        ind = l[:len(l) - len(l.lstrip())]
        if i > draw_at and todo_d and s.startswith(todo_d[0][0]):
            anchor, name = todo_d.pop(0)
            if name is None:
                out.append(f'{ind}crate::modern_perf_hook::frame_begin(device, queue);')
                out.append(f'{ind}crate::modern_perf_hook::phase("targets");')
            else:
                out.append(f'{ind}crate::modern_perf_hook::phase("{name}");')
            if anchor.startswith('self.encode('):
                out.append(f'{ind}let commands = {s};')
                out.append(f'{ind}crate::modern_perf_hook::frame_end(encoder);')
                out.append(f'{ind}commands')
            else:
                out.append(l)
            continue
        if i > enc_at and todo_e and s.startswith(todo_e[0][0]):
            out.append(f'{ind}crate::modern_perf_hook::phase("{todo_e.pop(0)[1]}");')
        if i > enc_at and s == UNIT:
            out.append(f'{ind}let _unit_time = crate::modern_perf_hook::unit_timer(decl.label);')
            units += 1
        out.append(l)
    if todo_d or todo_e or units < 2:
        sys.exit(f'modern_stage: missing anchors: {[a for a, _ in todo_d + todo_e] + ([UNIT] if units < 2 else [])}')
    t = '\n'.join(out)
    t = t.replace('\n#[cfg(test)]\nmod tests;', '\n#[cfg(test)]\nmod tests;\n#[cfg(test)]\nmod perf_bench;\n#[cfg(test)]\nmod perf_samples;', 1)
    assert 'mod perf_bench;' in t, 'frame/mod.rs anchor: mod tests;'
    rend.write_text(t)
(srcdir / 'frame/perf_bench.rs').write_text(
    owned_access((HERE / 'modern_perf_bench.rs').read_text(), 'r'))
# The anti-aliasing change as this tree's client makes it: the renderer's
# own `set_samples` where it has one, else a new renderer (the caller then
# re-applies its settings; returns whether the renderer was kept).
has_set_samples = 'pub fn set_samples(' in ''.join(p.read_text() for p in srcdir.rglob('*.rs'))
body = ('    r.set_samples(device, samples);\n    let _ = queue;\n    true\n' if has_set_samples else
        '    *r = ModernRenderer::new(device, queue, wgpu::TextureFormat::Rgba8Unorm, samples);\n    false\n')
# The bench's thread count (`MODERN_BENCH_THREADS`; 0 only reads it): this
# tree's `set_threads` where it has one (a tree without it is
# single-threaded).
has_prepare = 'pub fn prepare_sample_counts(' in ''.join(p.read_text() for p in srcdir.rglob('*.rs'))
has_set_threads = 'fn set_threads(' in ''.join(p.read_text() for p in srcdir.rglob('*.rs'))
threads_body = ('    if threads > 0 {\n        r.set_threads(threads);\n    }\n    r.threads()\n' if has_set_threads else
                '    let _ = (r, threads);\n    1\n')
(srcdir / 'frame/perf_samples.rs').write_text(
    '//! Generated by tools/perf/modern_stage.py.\nuse super::*;\n'
    'pub(super) fn change_samples(r: &mut ModernRenderer, device: &wgpu::Device, queue: &wgpu::Queue, samples: u32) -> bool {\n'
    + body + '}\n'
    'pub(super) fn set_threads(r: &mut ModernRenderer, threads: usize) -> usize {\n'
    + threads_body + '}\n'
    # The anti-aliasing change alone (lane P5's hitch bench): the renderer's
    # `set_samples`.
    'pub(super) fn set_samples_only(r: &mut ModernRenderer, device: &wgpu::Device, samples: u32) {\n'
    '    r.set_samples(device, samples);\n}\n'
    # The other sample counts' pipeline sets built ahead, as the client's
    # startup thread does (a tree without `prepare_sample_counts` builds
    # them at the change).
    'pub(super) fn prepare_counts(r: &mut ModernRenderer, device: &wgpu::Device, queue: &wgpu::Queue, counts: &[u32]) {\n'
    + ('    r.prepare_sample_counts(device, queue, counts);\n' if has_prepare else '    let _ = (r, device, queue, counts);\n')
    + '}\n')

subprocess.run(['rsync', '-rlc', '--delete', '--exclude', 'target', f'{stage}/tools/', f'{dest}/tools/'],
               check=True)
for link, target in (('server/data/pack', src / 'server/data/pack'), ('docs', src / 'docs')):
    p = dest / link
    p.parent.mkdir(parents=True, exist_ok=True)
    if not p.is_symlink():
        p.symlink_to(target.resolve() if target.exists() else target)
print(f'modern_stage: {dest}', file=sys.stderr)
