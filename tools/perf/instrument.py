#!/usr/bin/env python3
"""Instrument a COPY of the tree with the Phase 6 performance probe.

usage: instrument.py ROOT   (ROOT contains tools/client910; never the worktree)

- copies tools/perf/perf_probe.rs into rs910-gpu-device as `perf_probe`
  (plus the `libc` dependency it reads thread CPU time with);
- rewrites every wgpu creation/write/submit/pass call in rs910-render-gpu
  and rs910-gpu-device to its `tr_*` twin (textual and generic, by method
  name, so it applies to any revision of the renderer); `set_vertex_buffer(
  s, X.slice(R))` becomes `tr_set_vertex_buffer(s, &X, R)` (the probe needs
  the buffer and range, which a `BufferSlice` does not expose in wgpu);
- installs the probe's counting allocator in the client910 bin and times
  `about_to_wait` (logic) and `render_frame` (redraw) in app.rs, closing
  each redraw with `perf_probe::frame_end`.

The rewrite is idempotent (an instrumented file is left alone). It exits
non-zero when an expected anchor is missing, so a renamed call site is
noticed instead of silently untraced.
"""
import re
import shutil
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
root = Path(sys.argv[1]).resolve()
client = root / 'tools/client910'
dev = client / 'crates/rs910-gpu-device'
gpu = client / 'crates/rs910-render-gpu'
if (root / '.git').exists() and not (root / '.perf-copy').exists():
    sys.exit(f'refusing to instrument {root}: not a perf build copy (no .perf-copy marker)')

probe = (HERE / 'perf_probe.rs').read_text()
if (dev / 'src/uploads.rs').exists():
    # The renderer writes buffers through `uploads::Uploader` (the device's
    # staging belt, or the queue) and submits through `Device::submit`.
    probe += '''
impl<T: crate::uploads::Uploader + ?Sized> WriteTr for T {
    fn tr_write_buffer(&self, buffer: &wgpu::Buffer, offset: u64, data: &[u8]) {
        note_write(buffer, offset, data);
        time_write(|| crate::uploads::Uploader::write_buffer(self, buffer, offset, data));
    }
}
impl DeviceSubmitTr for crate::gpu_device::Device {
    fn tr_submit<I: IntoIterator<Item = wgpu::CommandBuffer>>(&self, buffers: I) -> wgpu::SubmissionIndex {
        self.submit(buffers)
    }
}
'''
else:
    probe += '''
impl WriteTr for wgpu::Queue {
    fn tr_write_buffer(&self, buffer: &wgpu::Buffer, offset: u64, data: &[u8]) {
        note_write(buffer, offset, data);
        time_write(|| self.write_buffer(buffer, offset, data));
    }
}
'''
(dev / 'src/perf_probe.rs').write_text(probe)
lib = dev / 'src/lib.rs'
t = lib.read_text()
if 'pub mod perf_probe;' not in t:
    t = t.replace('pub mod gpu_device;', 'pub mod gpu_device;\npub mod perf_probe;', 1)
    assert 'pub mod perf_probe;' in t, 'rs910-gpu-device lib.rs anchor'
    lib.write_text(t)
cargo = dev / 'Cargo.toml'
t = cargo.read_text()
if '\nlibc = ' not in t:
    t = t.replace('wgpu = "30"\n', 'wgpu = "30"\nlibc = "0.2"\n', 1)
    assert '\nlibc = ' in t, 'rs910-gpu-device Cargo.toml anchor'
    cargo.write_text(t)

SIMPLE = [
    'create_shader_module', 'create_bind_group_layout', 'create_pipeline_layout',
    'create_render_pipeline', 'create_render_bundle_encoder', 'create_buffer_init',
    'create_buffer', 'create_texture_with_data', 'create_texture', 'create_sampler',
    'create_bind_group', 'create_command_encoder', 'create_view', 'write_buffer',
    'write_texture', 'submit', 'begin_render_pass', 'copy_texture_to_texture',
    'copy_buffer_to_buffer', 'copy_texture_to_buffer', 'set_pipeline', 'set_bind_group',
    'draw_indexed', 'set_viewport', 'set_scissor_rect', 'execute_bundles',
]


def split_args(text, start):
    """Args of the call whose '(' is at text[start]; returns (args, end)."""
    depth = 0
    args, cur = [], []
    i = start
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


def unslice(arg, where):
    m = re.fullmatch(r'(?s)(.*)\.slice\((.*)\)', arg)
    if not m:
        sys.exit(f'{where}: buffer argument is not `X.slice(R)`: {arg!r}')
    return f'&{m.group(1)}', m.group(2)


def rewrite_slices(s, where):
    for name in ('set_vertex_buffer', 'set_index_buffer'):
        out, pos = [], 0
        for m in re.finditer(r'\.(\s*)' + name + r'\(', s):
            if m.start() < pos:
                continue
            args, end = split_args(s, m.end() - 1)
            if name == 'set_vertex_buffer':
                buf, rng = unslice(args[1], where)
                new = f'.{m.group(1)}tr_{name}({args[0]}, {buf}, {rng})'
            else:
                buf, rng = unslice(args[0], where)
                new = f'.{m.group(1)}tr_{name}({buf}, {rng}, {args[1]})'
            out.append(s[pos:m.start()])
            out.append(new)
            pos = end
        out.append(s[pos:])
        s = ''.join(out)
    return s


total = 0
for crate, path in ((dev, 'crate::perf_probe'), (gpu, 'rs910_gpu_device::perf_probe')):
    for f in sorted((crate / 'src').rglob('*.rs')):
        if f.name in ('perf_probe.rs', 'uploads.rs', 'tests.rs') or f.name.endswith('_tests.rs'):
            continue
        s = f.read_text()
        if 'perf_probe::{' in s:
            continue
        orig = s
        for name in SIMPLE:
            s = re.sub(r'\.(\s*)' + name + r'\(', r'.\1tr_' + name + '(', s)
        s = re.sub(r'\b(pass|encoder)(\s*)\.(\s*)draw\((?!\s*&)', r'\1\2.\3tr_draw(', s)
        s = re.sub(r'\.(\s*)finish\(&wgpu::RenderBundleDescriptor',
                   r'.\1tr_finish(&wgpu::RenderBundleDescriptor', s)
        s = rewrite_slices(s, f)
        if s != orig:
            n = sum(1 for a, b in zip(orig.split('('), s.split('(')) if a != b)
            s += ('\n#[allow(unused_imports)]\nuse ' + path + '::{BundleTr as _, DeviceTr as _, '
                  'EncoderTr as _, PassTr as _, QueueTr as _, RpassTr as _, TextureTr as _, WriteTr as _, '
                  'DeviceSubmitTr as _};\n')
            f.write_text(s)
            total += 1
print(f'instrumented {total} files')

# In-process reuse check (CLIENT910_PERF_CHECK_REUSE with the trace on):
# every transient mesh reused for a new model is compared, byte for byte
# through the trace's buffer shadows, with a fresh `from_model` build of the
# same model (never drawn), when the tree reuses transient meshes.
fr = gpu / 'src/floor_render/mesh.rs'
if not fr.exists():
    fr = gpu / 'src/floor_render.rs'
t = fr.read_text()
if 'pub fn reuse_for_model' in t and 'probe_same_as' not in t:
    t += '''
impl FloorMesh {
    /// perf probe: `self` (a reused mesh) against `fresh` (`from_model`).
    pub fn probe_same_as(&self, fresh: &FloorMesh) -> Result<(), String> {
        use rs910_gpu_device::perf_probe::buffer_contents as bytes;
        let eq = |what: &str, a: Option<Vec<u8>>, b: Option<Vec<u8>>| {
            if a.is_none() || a != b {
                Err(format!("{what}: {:?} vs {:?}", a.map(|v| v.len()), b.map(|v| v.len())))
            } else {
                Ok(())
            }
        };
        eq("vertices", bytes(&self.vertex_buffer), bytes(&fresh.vertex_buffer))?;
        eq(
            "colours",
            self.shared_colours.as_ref().and_then(bytes),
            fresh.shared_colours.as_ref().and_then(bytes),
        )?;
        if (self.vertex_count, self.depth_write, self.model_uniform.is_some(), self.batches.len())
            != (fresh.vertex_count, fresh.depth_write, fresh.model_uniform.is_some(), fresh.batches.len())
        {
            return Err("mesh fields".into());
        }
        if format!("{:?}", self.billboards) != format!("{:?}", fresh.billboards) {
            return Err("billboards".into());
        }
        for (i, (a, b)) in self.batches.iter().zip(&fresh.batches).enumerate() {
            if (a.index_count, a.material, a.alpha_test, a.floor_batch, a.colour_buffer.is_some())
                != (b.index_count, b.material, b.alpha_test, b.floor_batch, b.colour_buffer.is_some())
            {
                return Err(format!("batch {i} fields"));
            }
            eq(&format!("batch {i} indices"), bytes(&a.index_buffer), bytes(&b.index_buffer))?;
            let (pa, pb) = (a.payload.as_ref().unwrap(), b.payload.as_ref().unwrap());
            if bytemuck::bytes_of(&pa.values) != bytemuck::bytes_of(&pb.values) || pa.spec != pb.spec {
                return Err(format!("batch {i} payload"));
            }
            eq(&format!("batch {i} uniforms"), bytes(&pa.buffer), bytes(&pb.buffer))?;
        }
        Ok(())
    }
}
'''
    fr.write_text(t)
sm = gpu / 'src/scene_meshes.rs'
t = sm.read_text()
anchor = '''                Some(mut mesh) => renderer
                    .reuse_model_mesh(gpu, &mut mesh, materials, model, origin)?
                    .then_some(mesh),'''
if anchor in t:
    t = t.replace(anchor, '''                Some(mut mesh) => {
                    let reused = renderer.reuse_model_mesh(gpu, &mut mesh, materials, model, origin)?;
                    if reused && rs910_gpu_device::perf_probe::check_reuse() {
                        let fresh = renderer.build_model_mesh(gpu, pack, materials, model, origin, "probe")?;
                        rs910_gpu_device::perf_probe::reuse_checked(mesh.probe_same_as(&fresh));
                    }
                    reused.then_some(mesh)
                }''', 1)
    sm.write_text(t)

main = client / 'src/main.rs'
t = main.read_text()
if 'CountingAlloc' not in t:
    t += ('\n#[global_allocator]\nstatic PERF_ALLOC: rs910_gpu_device::perf_probe::CountingAlloc =\n'
          '    rs910_gpu_device::perf_probe::CountingAlloc;\n')
    main.write_text(t)

app = client / 'src/app.rs'
t = app.read_text()
if 'perf_probe' not in t:
    a = 'fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {\n'
    assert t.count(a) == 1, 'app.rs about_to_wait anchor'
    t = t.replace(a, a + '        let _perf = rs910_gpu_device::perf_probe::Segment::begin(0);\n', 1)
    # The redraw call (`self.render_frame();`, or wrapped in the engine
    # profiler's `profile::scope!("redraw", ...)` since lane E-A4).
    calls = re.findall(r'^ +[^\n]*self\.render_frame\(\)[^\n]*\n', t, re.M)
    assert len(calls) == 1, 'app.rs render_frame anchor'
    r = calls[0]
    t = t.replace(r, (
        '                let perf = rs910_gpu_device::perf_probe::Segment::begin(1);\n'
        + r +
        '                drop(perf);\n'
        '                rs910_gpu_device::perf_probe::frame_end(\n'
        '                    self.renderer.as_ref().map(|r| &r.faithful_ref().1.device),\n'
        '                );\n'), 1)
    app.write_text(t)
print('ok')
