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
  | { type: "SaveProject"; path?: string | null }
  | { type: "AddMessage"; message: MessageDecl }
  | { type: "UpdateMessage"; index: number; message: MessageDecl }
  | { type: "RemoveMessage"; index: number }
  | { type: "StartRenodeRun"; path: string; runSecs?: number }
  | { type: "GetRenodeJob"; jobId: number }
  | { type: "ListRenodeJobs" }
  | { type: "ImportRenodeTrace"; jobId: number };

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
  | { ArbitrationStarted: { contenders: string[] } }
  | { ArbitrationLost: { node: string; winner: string } }
  | { FrameTransmitted: { sender: string; frame: WireCanFrame } }
  | { FrameReceived: { receiver: string; frame: WireCanFrame } }
  | { CanError: { node: string; kind: string } }
  | { ErrorStateChanged: { node: string; state: string } }
  | { FrameDropped: { sender: string; frame: WireCanFrame; fault: unknown } };

export type SimEventKind =
  | "SimulationStarted"
  | "SimulationPaused"
  | "SimulationStopped"
  | "SimulationReset"
  | { NodeRegistered: { node: string } }
  | { BusTraffic: BusEventKind };

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
}

/** One scripted frame (mirror of MessageDecl in src/project/schema.rs). */
export interface MessageDecl {
  sender: string;
  id: number;
  data: number[];
  extended?: boolean;
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

/** One background firmware run (mirror of JobInfo in src/server/proto.rs). */
export interface RenodeJob {
  jobId: number;
  project: string;
  runSecs: number;
  state: "running" | "done" | "failed";
  transmitted: number;
  received: number;
  error: string | null;
}

export type ServerMsg =
  | { type: "Status"; state: EngineState; projectPath: string | null; buses: string[]; nodes: string[]; nextSeq: number; dirty: boolean }
  | { type: "Events"; events: SeqEvent[]; nextSeq: number }
  | { type: "Project"; project: ProjectDoc }
  | { type: "RunSummary"; transmitted: number; received: number; nextSeq: number }
  | { type: "Stepped"; nowNs: number; nextSeq: number }
  | { type: "Injected"; receivers: number; nextSeq: number }
  | { type: "RenodeJobStarted"; jobId: number }
  | { type: "RenodeJob"; job: RenodeJob }
  | { type: "RenodeJobList"; jobs: RenodeJob[] }
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
  dir: "TX" | "RX";
  node: string;
  id: string;
  dlc: number;
  data: string;
}

export function analyzerRows(events: SeqEvent[]): AnalyzerRow[] {
  const rows: AnalyzerRow[] = [];
  for (const e of events) {
    if (typeof e.kind !== "object" || !("BusTraffic" in e.kind)) continue;
    const traffic = e.kind.BusTraffic;
    if ("FrameTransmitted" in traffic) {
      const { sender, frame } = traffic.FrameTransmitted;
      rows.push({ seq: e.seq, timeNs: e.timeNs, dir: "TX", node: sender, id: idToHex(frame.id), dlc: frame.dlc, data: bytesToHex(frame.data) });
    } else if ("FrameReceived" in traffic) {
      const { receiver, frame } = traffic.FrameReceived;
      rows.push({ seq: e.seq, timeNs: e.timeNs, dir: "RX", node: receiver, id: idToHex(frame.id), dlc: frame.dlc, data: bytesToHex(frame.data) });
    }
  }
  return rows;
}
