/**
 * CanLab WebSocket API client — TypeScript mirror of src/server/proto.rs.
 * Keep the two in sync by hand: tagged-union `type` field, camelCase wire
 * names, sequence-numbered event polling.
 */

export type EngineState = "idle" | "running" | "paused" | "stopped";

export type ClientMsg =
  | { type: "Load"; path: string }
  | { type: "Start" }
  | { type: "Pause" }
  | { type: "Resume" }
  | { type: "Stop" }
  | { type: "Reset" }
  | { type: "Step"; deltaNs: number }
  | { type: "Inject"; sender: string; id: number; extended?: boolean; data?: number[] }
  | { type: "Arbitrate"; frames: ArbitrateFrame[] }
  | { type: "InspectFrame"; id: number; extended?: boolean; data?: number[]; remote?: boolean; dlc?: number }
  | { type: "DecodeFrame"; dbc: string; id: number; data?: number[] }
  | { type: "InjectFault"; sender: string; id: number; extended?: boolean; data?: number[]; fault: WireFault }
  | { type: "GetEvents"; sinceSeq?: number }
  | { type: "GetStatus" }
  | { type: "GetProject" }
  | { type: "Ping" }
  | { type: "NewProject" }
  | { type: "AddBus"; id: string; bitrate: number }
  | { type: "UpdateBus"; id: string; bitrate: number }
  | { type: "RemoveBus"; id: string }
  | { type: "AddNode"; node: NodeDecl }
  | { type: "UpdateNode"; id: string; node: NodeDecl }
  | { type: "RemoveNode"; id: string }
  | { type: "DisableNode"; id: string }
  | { type: "EnableNode"; id: string }
  | { type: "SaveProject"; path?: string | null }
  | { type: "AddMessage"; message: MessageDecl }
  | { type: "UpdateMessage"; index: number; message: MessageDecl }
  | { type: "RemoveMessage"; index: number }
  | { type: "ImportSketch"; filename: string; content: string; dialect?: string | null }
  | { type: "AddFault"; fault: FaultDecl }
  | { type: "UpdateFault"; index: number; fault: FaultDecl }
  | { type: "RemoveFault"; index: number }
  | { type: "StartRenodeRun"; path: string; runSecs?: number }
  | { type: "GetRenodeJob"; jobId: number }
  | { type: "CancelRenodeJob"; jobId: number }
  | { type: "GetRenodeLog"; jobId: number; lastN?: number }
  | { type: "ListRenodeJobs" }
  | { type: "ImportRenodeTrace"; jobId: number };

/** Wire shape of WireFault (externally-tagged Rust enum, Phase 9). */
export type WireFault = { FlipBit: number } | "CorruptCrc" | "DropFrame";

/** One DBC-decoded signal (mirror of DecodedSignal). */
export interface DecodedSignal {
  name: string;
  value: number;
  unit: string;
}

/** One named bit-region of an inspected frame. */
export interface BitRegion {
  name: string;
  offset: number;
  bits: string;
}

/** Bit-level layout of one frame (mirror of FrameBits). */
export interface FrameBits {
  idHex: string;
  extended: boolean;
  dlc: number;
  remote: boolean;
  regions: BitRegion[];
  crcHex: string;
  stuffBits: number;
  wireBits: number;
  wire: string;
}

/** One contender in an Arbitrate round (mirror of ArbitrateFrame). */
export interface ArbitrateFrame {
  sender: string;
  id: number;
  extended?: boolean;
  data?: number[];
}

/** Detection kind observed on a faulted wire (unit variants). */
export type CanErrorKind = "Bit" | "Stuff" | "Crc" | "Form" | "Ack";

/** Wire shape of CanId (externally-tagged Rust enum). */
export type WireCanId = { Standard: number } | { Extended: number };

export interface WireCanFrame {
  id: WireCanId;
  is_remote: boolean;
  dlc: number;
  data: number[];
  format: "Classical" | "Fd";
}

export type BusEventKind =
  | { NodeRegistered: { node: string } }
  | { NodeUnregistered: { node: string } }
  | { NodeReset: { node: string } }
  | { NodeDisabled: { node: string } }
  | { NodeEnabled: { node: string } }
  | { ArbitrationStarted: { contenders: string[] } }
  | { ArbitrationLost: { node: string; winner: string } }
  | { FrameTransmitted: { sender: string; frame: WireCanFrame } }
  | { FrameReceived: { receiver: string; frame: WireCanFrame } }
  | { CanError: { node: string; kind: string } }
  | { ErrorStateChanged: { node: string; state: string } }
  | { FrameDropped: { sender: string; frame: WireCanFrame; fault: unknown } };

/** Wire shape of BusEvent (struct): SimEventKind.BusTraffic carries the
 *  whole event struct, whose `kind` holds the variant. */
export interface BusEvent {
  time_ns: number;
  kind: BusEventKind;
}

export type SimEventKind =
  | "SimulationStarted"
  | "SimulationPaused"
  | "SimulationStopped"
  | "SimulationReset"
  | { NodeRegistered: { node: string } }
  | { BusTraffic: BusEvent };

export interface SeqEvent {
  seq: number;
  timeNs: number;
  kind: SimEventKind;
}

/** Project document shapes (mirror of src/project/schema.rs). */
export interface ProjectBus {
  id: string;
  type: string;
  bitrate: number;
  fd: boolean;
}

export interface ProjectNode {
  id: string;
  device: string;
  backend: string;
  firmware?: string | null;
  can: { bus: string };
  peripherals?: unknown[];
}

export interface ProjectDoc {
  version: number;
  simulation: { mode: string };
  buses: ProjectBus[];
  nodes: ProjectNode[];
  messages: MessageDecl[];
  faults: FaultDecl[];
}

/** One scripted frame (mirror of MessageDecl in src/project/schema.rs). */
export interface MessageDecl {
  sender: string;
  id: number;
  data: number[];
  extended?: boolean;
  /** Provenance for sketch-imported rows (`"node.ino:12"`); display-only. */
  source?: string | null;
}

/** A sketch value: resolved constant or runtime expression (never invented). */
export type Constness<T> = { Const: T } | { Dynamic: string };

/** One CAN send call-site from a sketch preview. */
export interface SketchSend {
  library: string;
  line: number;
  id: Constness<number>;
  extended: Constness<boolean>;
  dlc: Constness<number>;
  data: Constness<number>[];
}

/** One fault policy (mirror of FaultDecl in src/project/schema.rs). */
export interface FaultDecl {
  fault: WireFault;
  node?: string | null;
  id?: number | null;
  extended?: boolean;
  probability: number;
  seed: number;
}

/** Full node declaration as accepted by AddNode/UpdateNode. */
export interface NodeDecl {
  id: string;
  device: string;
  backend: string;
  firmware?: string | null;
  can: { bus: string };
  peripherals?: unknown[];
}

/** Component library (§25): the palette offer. */
export const KNOWN_DEVICES = [
  "stm32f103",
  "arduino_uno",
  "teensy41",
  "mcp2515",
  "can_analyzer",
  "generic_can_node",
];

/** One firmware UART line (mirror of UartLine in src/server/proto.rs). */
export interface UartLine {
  machine: string;
  message: string;
}

/** One background firmware run (mirror of JobInfo in src/server/proto.rs). */
export interface RenodeJob {
  jobId: number;
  project: string;
  runSecs: number;
  state: "running" | "done" | "failed" | "cancelled";
  transmitted: number;
  received: number;
  error: string | null;
}

export type ServerMsg =
  | { type: "Status"; state: EngineState; projectPath: string | null; buses: string[]; nodes: string[]; disabled: string[]; nextSeq: number; dirty: boolean }
  | { type: "Events"; events: SeqEvent[]; nextSeq: number }
  | { type: "Project"; project: ProjectDoc }
  | { type: "RunSummary"; transmitted: number; received: number; nextSeq: number }
  | { type: "Stepped"; nowNs: number; nextSeq: number }
  | { type: "Injected"; receivers: number; nextSeq: number }
  | { type: "Arbitrated"; winner: string; winnerId: number; winnerExtended: boolean; losers: string[]; receivers: number; nextSeq: number }
  | { type: "FaultInjected"; error: CanErrorKind | null; receivers: number; nextSeq: number }
  | { type: "RenodeJobStarted"; jobId: number }
  | { type: "RenodeJob"; job: RenodeJob }
  | { type: "RenodeJobList"; jobs: RenodeJob[] }
  | { type: "RenodeLog"; jobId: number; total: number; lines: UartLine[] }
  | { type: "SketchPreview"; detected: string[]; sends: SketchSend[] }
  | { type: "FrameBits"; idHex: string; extended: boolean; dlc: number; remote: boolean; regions: BitRegion[]; crcHex: string; stuffBits: number; wireBits: number; wire: string }
  | { type: "DecodedSignals"; idHex: string; message: string; signals: DecodedSignal[] }
  | { type: "TraceImported"; transmitted: number; received: number; nextSeq: number }
  | { type: "Pong" }
  | { type: "Error"; message: string };

/** One request in, exactly one reply out. Reconnects are the caller's job. */
export class CanLabApi {
  private ws: WebSocket | null = null;
  private pending: Array<{ resolve: (m: ServerMsg) => void; reject: (e: Error) => void }> = [];
  onClose: (() => void) | null = null;

  connect(url = "ws://127.0.0.1:21011"): Promise<void> {
    return new Promise((resolve, reject) => {
      const ws = new WebSocket(url);
      ws.onopen = () => {
        this.ws = ws;
        resolve();
      };
      ws.onerror = () => reject(new Error(`cannot reach ${url} — is 'canlab serve' running?`));
      ws.onmessage = (ev) => {
        const next = this.pending.shift();
        if (!next) return;
        try {
          next.resolve(JSON.parse(String(ev.data)) as ServerMsg);
        } catch (e) {
          next.reject(e instanceof Error ? e : new Error(String(e)));
        }
      };
      ws.onclose = () => {
        this.ws = null;
        this.onClose?.();
      };
    });
  }

  get connected(): boolean {
    return this.ws !== null && this.ws.readyState === WebSocket.OPEN;
  }

  request(msg: ClientMsg): Promise<ServerMsg> {
    return new Promise((resolve, reject) => {
      if (!this.ws || this.ws.readyState !== WebSocket.OPEN) {
        reject(new Error("not connected — start 'canlab serve' first"));
        return;
      }
      this.pending.push({ resolve, reject });
      this.ws.send(JSON.stringify(msg));
    });
  }

  close(): void {
    this.ws?.close();
    this.ws = null;
  }
}

/** "0x123" / "0x1FFFFFFF" rendering for the analyzer table. */
export function idToHex(id: WireCanId): string {
  if ("Standard" in id) return `0x${id.Standard.toString(16).toUpperCase()}`;
  return `0x${id.Extended.toString(16).toUpperCase()}`;
}

/** Structured ID filter (§31 subset): exact, inclusive range, or mask.
 *  Pure (no DOM) so it stays unit-testable outside the browser. */
export type IdFilterSpec =
  | { kind: "all" }
  | { kind: "exact"; id: number }
  | { kind: "range"; lo: number; hi: number }
  | { kind: "mask"; id: number; mask: number }
  | { kind: "invalid"; message: string };

function parseHexNum(text: string): number | null {
  const t = text.trim().toLowerCase().startsWith("0x") ? text.trim() : `0x${text.trim()}`;
  const n = Number(t);
  return Number.isInteger(n) && n >= 0 ? n : null;
}

export function parseIdFilter(text: string): IdFilterSpec {
  const t = text.trim();
  if (t === "") return { kind: "all" };
  if (t.includes("-")) {
    const [a, b] = t.split("-", 2).map((s) => parseHexNum(s));
    if (a === null || b === null) return { kind: "invalid", message: `"${text}" is not a range (e.g. 0x100-0x2FF)` };
    if (a > b) return { kind: "invalid", message: `range start 0x${a.toString(16).toUpperCase()} exceeds end 0x${b.toString(16).toUpperCase()}` };
    return { kind: "range", lo: a, hi: b };
  }
  if (t.includes("/")) {
    const [a, b] = t.split("/", 2).map((s) => parseHexNum(s));
    if (a === null || b === null) return { kind: "invalid", message: `"${text}" is not an id/mask pair (e.g. 0x120/0x7F0)` };
    return { kind: "mask", id: a, mask: b };
  }
  const id = parseHexNum(t);
  if (id === null) return { kind: "invalid", message: `"${text}" is not a hex id, range, or id/mask pair` };
  return { kind: "exact", id };
}

/** Numeric frame id (from an "0x123" row id) against a parsed spec.
 *  Invalid specs match nothing — the UI shows the parse error instead. */
export function matchIdFilter(idNum: number, spec: IdFilterSpec): boolean {
  switch (spec.kind) {
    case "all": return true;
    case "exact": return idNum === spec.id;
    case "range": return idNum >= spec.lo && idNum <= spec.hi;
    case "mask": return (idNum & spec.mask) === (spec.id & spec.mask);
    case "invalid": return false;
  }
}

export function bytesToHex(data: number[]): string {
  return data.map((b) => b.toString(16).toUpperCase().padStart(2, "0")).join(" ");
}

/** Nanoseconds → "0.238 ms" display (matches CLI fmt_time). */
export function fmtTimeNs(ns: number): string {
  return `${(ns / 1_000_000).toFixed(3)} ms`;
}

/** Extract analyzer rows (TX/RX frame deliveries) from polled events. */
export interface AnalyzerRow {
  seq: number;
  timeNs: number;
  dir: "TX" | "RX" | "DROP" | "ARB";
  node: string;
  id: string;
  /** True for 29-bit extended identifiers. */
  extended: boolean;
  dlc: number;
  data: string;
  /** Remote (RTR) frames carry no payload; dlc is the requested length. */
  remote: boolean;
}

/** Extract analyzer rows (TX/RX/DROP/ARB) from polled events.
 *  Wire nesting is SeqEvent.kind = { BusTraffic: BusEvent } and
 *  BusEvent.kind carries the variant — not the variant directly.
 *  ARB rows annotate arbitration losses: the loser node plus the winning
 *  frame (resolved from the winner's TX at the same instant). */
export function analyzerRows(events: SeqEvent[]): AnalyzerRow[] {
  const txBySenderTime = new Map<string, WireCanFrame>();
  for (const e of events) {
    if (typeof e.kind !== "object" || !("BusTraffic" in e.kind)) continue;
    const inner = (e.kind.BusTraffic as BusEvent | null)?.kind;
    if (!inner || typeof inner !== "object" || !("FrameTransmitted" in inner)) continue;
    txBySenderTime.set(`${e.timeNs}:${inner.FrameTransmitted.sender}`, inner.FrameTransmitted.frame);
  }
  const rows: AnalyzerRow[] = [];
  for (const e of events) {
    if (typeof e.kind !== "object" || !("BusTraffic" in e.kind)) continue;
    const inner = (e.kind.BusTraffic as BusEvent | null)?.kind;
    if (!inner || typeof inner !== "object") continue;
    if ("FrameTransmitted" in inner) {
      const { sender, frame } = inner.FrameTransmitted;
      rows.push({ seq: e.seq, timeNs: e.timeNs, dir: "TX", node: sender, id: idToHex(frame.id), extended: "Extended" in frame.id, dlc: frame.dlc, data: bytesToHex(frame.data), remote: frame.is_remote });
    } else if ("FrameReceived" in inner) {
      const { receiver, frame } = inner.FrameReceived;
      rows.push({ seq: e.seq, timeNs: e.timeNs, dir: "RX", node: receiver, id: idToHex(frame.id), extended: "Extended" in frame.id, dlc: frame.dlc, data: bytesToHex(frame.data), remote: frame.is_remote });
    } else if ("FrameDropped" in inner) {
      const { sender, frame } = inner.FrameDropped;
      rows.push({ seq: e.seq, timeNs: e.timeNs, dir: "DROP", node: sender, id: idToHex(frame.id), extended: "Extended" in frame.id, dlc: frame.dlc, data: bytesToHex(frame.data), remote: frame.is_remote });
    } else if ("ArbitrationLost" in inner) {
      const { node, winner } = inner.ArbitrationLost;
      const frame = txBySenderTime.get(`${e.timeNs}:${winner}`);
      if (!frame) continue; // defensive: winner TX always shares the instant
      rows.push({ seq: e.seq, timeNs: e.timeNs, dir: "ARB", node, id: idToHex(frame.id), extended: "Extended" in frame.id, dlc: frame.dlc, data: bytesToHex(frame.data), remote: frame.is_remote });
    }
  }
  return rows;
}

/** PCAP export (§47, SocketCAN link type): analyzer rows as one
 *  `DLT_CAN_SOCKETCAN` (227) capture with simulated timestamps.
 *  Pure bytes (no DOM) so a script can parse them back and verify. */
export const PCAP_DLT_CAN_SOCKETCAN = 227;
const CAN_EFF_FLAG = 0x80000000;
const CAN_RTR_FLAG = 0x40000000;

export function rowsToPcap(rows: AnalyzerRow[]): Uint8Array<ArrayBuffer> {
  const out: number[] = [];
  const le32 = (n: number) => {
    out.push(n & 0xff, (n >>> 8) & 0xff, (n >>> 16) & 0xff, (n >>> 24) & 0xff);
  };
  const le16 = (n: number) => {
    out.push(n & 0xff, (n >>> 8) & 0xff);
  };
  // Global header, little-endian magic.
  le32(0xa1b2c3d4);
  le16(2);
  le16(4);
  le32(0); // thiszone
  le32(0); // sigfigs
  le32(72); // snaplen
  le32(PCAP_DLT_CAN_SOCKETCAN);
  for (const r of rows) {
    // ARB rows annotate losses, not wire traffic: the winner's TX already
    // exports the frame. (DROP rows export as normal frames, by contrast.)
    if (r.dir === "ARB") continue;
    const idNum = Number(r.id);
    if (!Number.isInteger(idNum) || idNum < 0 || idNum > 0x1fffffff) continue;
    const dataBytes =
      r.data.trim() === "" ? [] : r.data.trim().split(/\s+/).map((b) => Number(`0x${b}`));
    if (dataBytes.some((b) => !Number.isInteger(b) || b < 0 || b > 0xff)) continue;
    const sec = Math.floor(r.timeNs / 1e9);
    const usec = Math.floor((r.timeNs % 1e9) / 1000);
    le32(sec);
    le32(usec);
    le32(16); // incl_len: struct can_frame
    le32(16); // orig_len
    let canId = idNum;
    if (idNum > 0x7ff) canId |= CAN_EFF_FLAG;
    if (r.remote) canId |= CAN_RTR_FLAG;
    le32(canId >>> 0);
    out.push(r.dlc & 0xff, 0, 0, 0); // dlc + 3 pad bytes
    for (let i = 0; i < 8; i++) out.push(dataBytes[i] ?? 0);
  }
  return new Uint8Array(out);
}

/** Fixed tail of every transmitted wire: CRC delimiter + ACK slot +
 *  ACK delimiter + EOF(7). Always the last 10 wire bits. */
export const WAVEFORM_TAIL_BITS = 10;

export interface WaveformBand {
  name: "SOF" | "Stuffed" | "Tail";
  start: number;
  len: number;
}

/** Band split of a transmitted wire for the inspector waveform strip
 *  (Phase 10, slice 1): SOF cell, the stuffed SOF..CRC middle, and the
 *  fixed tail. Per-region stuffed offsets are NOT recoverable from the
 *  wire alone (stuffing runs cross region boundaries), so the middle
 *  stays one band. Pure (no DOM) for harness testing. */
export function waveformBands(wireBits: number): WaveformBand[] {
  if (!Number.isInteger(wireBits) || wireBits < 1 + WAVEFORM_TAIL_BITS) return [];
  return [
    { name: "SOF", start: 0, len: 1 },
    { name: "Stuffed", start: 1, len: wireBits - 1 - WAVEFORM_TAIL_BITS },
    { name: "Tail", start: wireBits - WAVEFORM_TAIL_BITS, len: WAVEFORM_TAIL_BITS },
  ];
}
