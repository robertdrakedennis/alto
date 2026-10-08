/** Owned authenticated transport adapter. No world/player/NPC state mutations.
 * Four real WireClients own four separate login ciphers. Four separate ordinary
 * Rust cores decode their own unmasked received bytes and produce their own
 * native input frames. The outer recorder owns/spawns/waits the Rust children.
 */
import fs from 'node:fs';
import net from 'node:net';
import {randomBytes, createHash} from 'node:crypto';
import {pathToFileURL} from 'node:url';

const FIRST = 0;
const ONE = 1;
const SEED_WORDS = 4;
const WORD_BYTES = 4;
const BYTE_BITS = 8;
const OPCODE_SHORT_MARK = 128;
const OPCODE_SHORT_SHIFT = 8;
const VARIABLE_BYTE_SIZE = -1;
const VARIABLE_SHORT_SIZE = -2;
const BYTE_LENGTH = 1;
const SHORT_LENGTH = 2;
const LINE_BYTES = 2 * 1024 * 1024;
const PENDING_BYTES = 4 * 1024 * 1024;
const MAX_QUEUED_MESSAGES = 4096;
const HANDSHAKE_MS = 30_000;
const SESSION_MS = 1_800_000;
const CLOSE_MS = 5_000;
const SETTLE_MS = 30_000;

/** Track the underlying promise as well as its bounded waiter. A timed-out
 * operation is never silently called settled; cleanup exposes any tail. */
class OwnedPromises {
    constructor() { this.active = new Map(); this.completed = FIRST; this.serial = FIRST; }
    track(promise, label) {
        const identity = this.serial++;
        const value = Promise.resolve(promise);
        this.active.set(identity, {label, promise: value});
        value.then(() => this.finish(identity), () => this.finish(identity));
        return value;
    }
    finish(identity) { if (this.active.delete(identity)) this.completed += ONE; }
    wait(promise, milliseconds, label) { return bounded(this.track(promise, label), milliseconds, label); }
    async settle() {
        await bounded(Promise.allSettled([...this.active.values()].map(row => row.promise)), SETTLE_MS, 'owned socket promise settlement');
    }
    summary() { return {completed: this.completed, pending: [...this.active.values()].map(row => row.label)}; }
}

function sha(bytes) { return createHash('sha256').update(bytes).digest('hex'); }
function checked(spec) {
    if (sha(fs.readFileSync(spec.path)) !== spec.sha256) throw new Error(`frozen module changed: ${spec.path}`);
    return pathToFileURL(spec.path).href;
}
function bounded(promise, milliseconds, label) {
    let timer;
    return Promise.race([promise, new Promise((_, reject) => {
        timer = setTimeout(() => reject(new Error(`${label} deadline`)), milliseconds);
    })]).finally(() => clearTimeout(timer));
}

/** Only framing is adapted. The original generated decoder owns payloads. */
function serverWire(frame, table) {
    const rule = table.get(frame.name);
    if (!rule || rule.opcode !== frame.opcode) throw new Error('received server table identity differs');
    const opcode = frame.opcode < OPCODE_SHORT_MARK
        ? [frame.opcode] : [OPCODE_SHORT_MARK + (frame.opcode >> OPCODE_SHORT_SHIFT), frame.opcode & ((ONE << BYTE_BITS) - ONE)];
    const bytes = Buffer.from(frame.payload);
    let length;
    if (rule.size === VARIABLE_BYTE_SIZE) {
        if (bytes.length >= ONE << BYTE_BITS) throw new Error('server byte-size overflow');
        length = [bytes.length];
    } else if (rule.size === VARIABLE_SHORT_SIZE) {
        if (bytes.length >= ONE << (BYTE_BITS * SHORT_LENGTH)) throw new Error('server short-size overflow');
        length = [bytes.length >> BYTE_BITS, bytes.length & ((ONE << BYTE_BITS) - ONE)];
    } else {
        if (bytes.length !== rule.size) throw new Error('server fixed-size framing differs');
        length = [];
    }
    return Buffer.concat([Buffer.from(opcode), Buffer.from(length), bytes]);
}

function clientFrames(wire, table) {
    const byOpcode = new Map([...table.values()].map(row => [row.opcode, row]));
    const bytes = Buffer.from(wire);
    const frames = [];
    let cursor = FIRST;
    while (cursor < bytes.length) {
        const start = cursor;
        const rule = byOpcode.get(bytes[cursor++]);
        if (!rule) throw new Error('core emitted unknown generated client opcode');
        let size = rule.size;
        if (size === VARIABLE_BYTE_SIZE) {
            if (cursor + BYTE_LENGTH > bytes.length) throw new Error('core byte-size prefix truncated');
            size = bytes[cursor++];
        } else if (size === VARIABLE_SHORT_SIZE) {
            if (cursor + SHORT_LENGTH > bytes.length) throw new Error('core short-size prefix truncated');
            size = bytes.readUInt16BE(cursor);
            cursor += SHORT_LENGTH;
        }
        if (size < FIRST || cursor + size > bytes.length) throw new Error('core client payload truncated');
        cursor += size;
        frames.push({name: rule.name, bytes: bytes.subarray(start, cursor)});
    }
    return frames;
}

class Bridge {
    constructor(spec, journal, promises) {
        this.spec = spec;
        this.journal = journal;
        this.promises = promises;
        this.rejectConnection = null;
        this.server = net.createServer();
        this.socket = null;
        this.messages = [];
        this.queuedBytes = FIRST;
        this.buffer = Buffer.alloc(FIRST);
        this.wakeup = null;
        this.closed = false;
        this.error = null;
    }
    async listen() {
        // Refuse replacing another socket; the recorder gives a fresh owned dir.
        if (fs.existsSync(this.spec.transport)) throw new Error('peer transport already exists');
        const connected = this.promises.track(new Promise((resolve, reject) => {
            this.rejectConnection = reject;
            this.server.once('error', reject);
            this.server.on('connection', socket => {
                if (this.socket) { socket.destroy(); return; }
                this.socket = socket;
                socket.on('data', bytes => {
                    try {
                        this.buffer = Buffer.concat([this.buffer, bytes]);
                        if (this.buffer.length > PENDING_BYTES) throw new Error('peer bridge pending bound');
                        let end;
                        while ((end = this.buffer.indexOf('\n')) >= FIRST) {
                            if (end > LINE_BYTES) throw new Error('peer bridge line bound');
                            this.queuedBytes += end + ONE;
                            if (this.queuedBytes > PENDING_BYTES || this.messages.length >= MAX_QUEUED_MESSAGES) throw new Error('peer bridge decoded queue bound');
                            this.messages.push({bytes: end + ONE, value: JSON.parse(this.buffer.subarray(FIRST, end).toString('utf8'))});
                            this.buffer = this.buffer.subarray(end + ONE);
                        }
                        this.wakeup?.();
                    } catch (error) { this.error = error; socket.destroy(); }
                });
                socket.on('error', error => { this.error = error; this.wakeup?.(); });
                socket.on('close', () => { this.closed = true; this.wakeup?.(); });
                resolve();
            });
        }), 'ordinary peer core connection');
        await this.promises.wait(new Promise((resolve, reject) => {
            this.server.once('error', reject);
            this.server.listen(this.spec.transport, resolve);
        }), HANDSHAKE_MS, 'bridge listen');
        fs.chmodSync(this.spec.transport, 0o600);
        this.identity = fs.lstatSync(this.spec.transport);
        this.journal({kind: 'peer-transport-listening', account: this.spec.name.toLowerCase(), transport: this.spec.transport, control: this.spec.control, rendered: false});
        await this.promises.wait(connected, HANDSHAKE_MS, 'ordinary peer core connection');
    }
    async send(row) {
        if (!this.socket || this.closed) throw new Error('peer bridge closed');
        const line = Buffer.from(JSON.stringify(row) + '\n');
        if (line.length > LINE_BYTES || this.socket.writableLength + line.length > PENDING_BYTES) throw new Error('peer bridge outbound bound');
        await this.promises.wait(new Promise((resolve, reject) => this.socket.write(line, error => error ? reject(error) : resolve())), HANDSHAKE_MS, 'bridge write');
    }
    async message(deadline) {
        while (!this.messages.length) {
            if (this.error) throw this.error;
            if (this.closed) {
                if (this.buffer.length) throw new Error('peer core truncated final line');
                return null;
            }
            await this.promises.wait(new Promise(resolve => { this.wakeup = resolve; }), Math.max(ONE, Math.min(HANDSHAKE_MS, deadline - Date.now())), 'bridge response');
            this.wakeup = null;
        }
        const row = this.messages.shift();
        this.queuedBytes -= row.bytes;
        return row.value;
    }
    async close() {
        this.closed = true;
        this.rejectConnection?.(new Error('owned peer bridge closing'));
        this.wakeup?.();
        this.socket?.destroy();
        try {
            if (this.server.listening) await this.promises.wait(new Promise(resolve => this.server.close(resolve)), CLOSE_MS, 'bridge close');
        } finally {
            if (this.identity && fs.existsSync(this.spec.transport)) {
                const now = fs.lstatSync(this.spec.transport);
                if (now.dev === this.identity.dev && now.ino === this.identity.ino) fs.unlinkSync(this.spec.transport);
            }
        }
    }
}

/** Called once by the passive fixture AFTER real Root waiting admission.
 * spec.modules names checked owning testkit/login modules; no copied corpus.
 */
export async function runPeer(spec, record, signal) {
    const protocol = await import(checked(spec.modules.socketTestkit));
    const {loginFor} = await import(checked(spec.modules.loginProvider));
    const {loginCryptoKey} = await import(checked(spec.modules.loginCrypto));
    const key = loginCryptoKey();
    if (!key || process.env.ALTO_LOGIN_CRYPTO === 'off') throw new Error('ordinary encrypted login is required');
    const promises = new OwnedPromises();
    const bridge = new Bridge(spec, record, promises);
    let client = null;
    const tasks = [];
    let stopping = false;
    let booted = false;
    let closing = false;
    let coreClosed = false;
    let resolveCoreClosed;
    let gracefulClose = null;
    let gracefulError = null;
    const coreClosure = new Promise(resolve => { resolveCoreClosed = resolve; });
    const deadline = Date.now() + SESSION_MS;
    const stop = () => { stopping = true; client?.socket.destroy(); bridge.closed = true; bridge.rejectConnection?.(new Error('owned peer aborted')); bridge.wakeup?.(); bridge.socket?.destroy(); };
    const requestClose = () => {
        if (!booted || !bridge.socket || bridge.closed) { stop(); return; }
        if (closing) return;
        closing = true;
        gracefulClose = promises.track((async () => {
            // The ordinary core flushes its final output before acknowledging.
            await bridge.send({kind: 'close'});
            await bounded(coreClosure, CLOSE_MS, 'ordinary core close acknowledgement');
        })().catch(error => { gracefulError = String(error); stop(); }), 'ordinary core graceful close');
    };
    signal.addEventListener('abort', requestClose, {once: true});
    try {
        if (signal.aborted) throw new Error('peer startup already aborted');
        await bridge.listen();
        const connecting = promises.track(protocol.WireClient.connect(spec.port), 'owned peer TCP connection');
        connecting.then(value => { if (stopping) value.socket.destroy(); }, () => {});
        client = await bounded(connecting, HANDSHAKE_MS, 'peer TCP connect');
        if (stopping) throw new Error('peer connection completed after cancellation');
        const seedBytes = randomBytes(SEED_WORDS * WORD_BYTES);
        const seeds = Array.from({length: SEED_WORDS}, (_, index) => seedBytes.readInt32BE(index * WORD_BYTES));
        const reply = await promises.wait(protocol.wireGameLogin(client, loginFor(spec.name),
            {key: {n: key.n, e: key.e}, seeds}), HANDSHAKE_MS, 'fresh encrypted peer login');
        if (reply.trailing !== FIRST || !reply.loggedInMembers) throw new Error('actual member profile differs');
        const blocks = reply.varcBlocks;
        if (!blocks.length || blocks.some((block, index) => block.final !== (index === blocks.length - ONE)
            || block.bytes[FIRST] !== Number(block.final))) throw new Error('actual server varc block order/flag differs');
        const frames = await promises.wait(client.tick(), HANDSHAKE_MS, 'fresh initial packet batch');
        const initial = Buffer.concat(frames.map(frame => serverWire(frame, protocol.RecordedServerProt)));
        const profile = {
            username: spec.name, pid: reply.uid, server_token: reply.serverToken.toString(),
            logged_in_members: reply.loggedInMembers, player_is_members: reply.playerIsMembers,
            player_is_quickchat: reply.playerIsQuickChat, logged_in_quickchat: reply.loggedInQuickChat,
            dob_verified: reply.dobVerified, lobby_dob: reply.lobbyDOB,
            staff_mod_level: reply.staffModLevel, player_mod_level: reply.playerModLevel, owner: reply.owner
        };
        record({kind: 'actual-peer-login', account: spec.name.toLowerCase(), profile,
                serverClock: reply.serverClock.toString(), varcBlocks: blocks,
                frames: frames.map(frame => ({name: frame.name, opcode: frame.opcode, payload: [...frame.payload]})),
                qualification: 'Own real RSA/XTEA/ISAAC login; tick-end batches are transport boundaries, never a logic clock.'});
        await bridge.send({kind: 'boot', boot: {device_envelope: spec.deviceEnvelope,
            profile, server_varcs: blocks.flatMap(block => block.bytes.slice(ONE)),
            startup_wire: [...initial], account: spec.name.toLowerCase(), server_clock: Number(reply.serverClock)}});
        // Process ordinary startup outputs before accepting the booted receipt.
        while (true) {
            const row = await bridge.message(deadline);
            if (!row) throw new Error('core closed during startup');
            if (row.kind === 'booted') {
                if (row.account !== spec.name.toLowerCase() || row.rendered !== false || !row.virtual_headless) throw new Error('peer backend identity differs');
                record(row);
                booted = true;
                break;
            }
            if (row.kind !== 'written') throw new Error('unexpected startup peer message');
            const startupFrames = clientFrames(row.wire, protocol.RecordedClientProt);
            if (startupFrames.some(frame => frame.name === 'CLIENT_CHEAT')) throw new Error('ordinary startup emitted forbidden cheat');
            for (const frame of startupFrames) client.writeFrame(frame.bytes);
            record({...row, account: spec.name.toLowerCase()});
        }
        let sequence = FIRST;
        tasks.push((async () => {
            while (!stopping && !closing && Date.now() < deadline) {
                const batch = await promises.wait(client.tick(), HANDSHAKE_MS, 'actual peer packet batch');
                if (closing) return;
                const wire = Buffer.concat(batch.map(frame => serverWire(frame, protocol.RecordedServerProt)));
                record({kind: 'actual-peer-received', account: spec.name.toLowerCase(), sequence,
                        frames: batch.map(frame => ({name: frame.name, opcode: frame.opcode, payload: [...frame.payload]}))});
                await bridge.send({kind: 'received', sequence, wire: [...wire]});
                sequence += ONE;
            }
        })());
        tasks.push((async () => {
            while (!stopping && Date.now() < deadline) {
                const row = await bridge.message(deadline);
                if (!row) return;
                if (row.kind === 'closed') {
                    if (!closing || row.account !== spec.name.toLowerCase())
                        throw new Error('unexpected ordinary core close acknowledgement');
                    coreClosed = true;
                    resolveCoreClosed();
                    return;
                }
                if (row.kind === 'written') {
                    const frames = clientFrames(row.wire, protocol.RecordedClientProt);
                    if (frames.some(frame => frame.name === 'CLIENT_CHEAT')) throw new Error('adaptive core emitted forbidden cheat');
                    for (const frame of frames) client.writeFrame(frame.bytes);
                    record({...row, account: spec.name.toLowerCase(), frames: frames.map(frame => frame.name)});
                } else if (row.kind === 'input' || row.kind === 'logic_cycle') record({...row, account: spec.name.toLowerCase()});
                else throw new Error('peer backend message kind changed');
            }
        })());
        // A single failed direction cancels both; finally observes every promise.
        for (const [index, task] of tasks.entries()) promises.track(task, `owned peer direction ${index}`);
        await Promise.race(tasks);
        if (!signal.aborted) throw new Error('peer backend ended before recorder-owned completion');
        if (gracefulClose) await gracefulClose;
        if (gracefulError) throw new Error(gracefulError);
    } finally {
        stop();
        signal.removeEventListener('abort', requestClose);
        // Every cleanup step runs even if an earlier evidence/close step fails.
        const errors = gracefulError ? [gracefulError] : [];
        try { await bridge.close(); } catch (error) { errors.push(String(error)); }
        try { if (client) await promises.wait(client.close(), CLOSE_MS, 'peer TCP finally close'); }
        catch (error) { errors.push(String(error)); }
        try { await promises.settle(); } catch (error) { errors.push(String(error)); }
        const summary = promises.summary();
        try { record({kind: 'peer-finally-closed', account: spec.name.toLowerCase(), rendered: false,
            socketPromises: summary, errors, coreClosed, closed: errors.length === FIRST && summary.pending.length === FIRST}); }
        catch (error) { errors.push(String(error)); }
        if (errors.length || summary.pending.length) throw new Error(`peer cleanup unqualified: ${JSON.stringify({errors, ...summary})}`);
    }
}
