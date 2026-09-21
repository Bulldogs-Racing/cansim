import React, { useCallback, useEffect, useRef, useState } from "react";
import ReactFlow, {
  applyNodeChanges,
  Background,
  Connection,
  Controls,
  Edge,
  Node,
  NodeChange,
} from "reactflow";
import "reactflow/dist/style.css";
import {
  AnalyzerRow,
  analyzerRows,
  CanLabApi,
  ClientMsg,
  EngineState,
  FaultDecl,
  fmtTimeNs,
  KNOWN_DEVICES,
  matchIdFilter,
  MessageDecl,
  NodeDecl,
  parseIdFilter,
  rowsToPcap,
  DecodedSignal,
  FrameBits,
  waveformBands,
  ProjectBus,
  ProjectNode,
  RenodeJob,
  SeqEvent,
  ServerMsg,
  SketchSend,
  UartLine,
  WireFault,
} from "./api";

const MAX_ROWS = 500; // ring buffer (§67): UI never grows without bound
const WS_URL = "ws://127.0.0.1:21011";

type StatusMsg = Extract<ServerMsg, { type: "Status" }>;

function replyError(reply: ServerMsg): string | null {
  return reply.type === "Error" ? reply.message : null;
}

function nextId(prefix: string, taken: string[]): string {
  let i = 1;
  while (taken.includes(`${prefix}-${i}`)) i++;
  return `${prefix}-${i}`;
}

function parseFlowId(id: string): { kind: "bus" | "node"; name: string } | null {
  if (id.startsWith("bus:")) return { kind: "bus", name: id.slice(4) };
  if (id.startsWith("node:")) return { kind: "node", name: id.slice(5) };
  return null;
}

/** Default canvas slots; real positions live in App and survive refreshes. */
function defaultPos(kind: "bus" | "node", index: number): { x: number; y: number } {
  if (kind === "bus") return { x: 260, y: index * 220 + 120 };
  const side = index % 2 === 0 ? -1 : 1;
  return { x: 260 + side * 320, y: Math.floor(index / 2) * 160 + 40 };
}

const BUS_STYLE: React.CSSProperties = { background: "#1d4ed8", color: "#fff", borderRadius: 8, padding: 10, width: 220, textAlign: "center" };
const NODE_STYLE: React.CSSProperties = { background: "#111827", color: "#e5e7eb", border: "1px solid #374151", borderRadius: 8, padding: 10, width: 200, textAlign: "center" };

export default function App(): JSX.Element {
  const apiRef = useRef<CanLabApi | null>(null);
  const cursorRef = useRef(0);
  const pausedRef = useRef(false);
  const positionsRef = useRef(new Map<string, { x: number; y: number }>());
  const [connected, setConnected] = useState(false);
  const [state, setState] = useState<EngineState>("idle");
  const [dirty, setDirty] = useState(false);
  const [projectPath, setProjectPath] = useState("examples/two_nodes.canlab.yaml");
  const [loadedPath, setLoadedPath] = useState<string | null>(null);
  const [buses, setBuses] = useState<ProjectBus[]>([]);
  const [nodes, setNodes] = useState<ProjectNode[]>([]);
  const [messages, setMessages] = useState<MessageDecl[]>([]);
  const [faults, setFaults] = useState<FaultDecl[]>([]);
  const [flowNodes, setFlowNodes] = useState<Node[]>([]);
  const [flowEdges, setFlowEdges] = useState<Edge[]>([]);
  const [canvasSel, setCanvasSel] = useState<string | null>(null);
  const [snapGrid, setSnapGrid] = useState(false);
  const [rows, setRows] = useState<AnalyzerRow[]>([]);
  const [filter, setFilter] = useState("");
  const [idFilterText, setIdFilterText] = useState("");
  const [newestFirst, setNewestFirst] = useState(false);
  const [dirFilter, setDirFilter] = useState<"all" | "TX" | "RX" | "DROP" | "ARB">("all");
  const [paused, setPaused] = useState(false);
  const [renodePath, setRenodePath] = useState("firmware/tests/stm32_can/two_nodes.canlab.yaml");
  const [runSecsText, setRunSecsText] = useState("30");
  const [jobs, setJobs] = useState<RenodeJob[]>([]);
  const [logJobId, setLogJobId] = useState<number | null>(null);
  const [logLines, setLogLines] = useState<UartLine[]>([]);
  const [logTotal, setLogTotal] = useState(0);
  const [selected, setSelected] = useState<AnalyzerRow | null>(null);
  const [bitLayout, setBitLayout] = useState<FrameBits | null>(null);
  const [dbcPath, setDbcPath] = useState("examples/vehicle.dbc");
  const [decoded, setDecoded] = useState<{ message: string; signals: DecodedSignal[] } | null>(null);
  const [disabledNodes, setDisabledNodes] = useState<string[]>([]);
  const [summary, setSummary] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const showError = useCallback((message: string) => setError(message), []);

  /** Adopt a Status reply: engine state, dirty chip, and event cursor. Rows
   *  past the cursor belong to a previous engine incarnation — drop them so
   *  the analyzer always reflects the current log. Skipped while capture is
   *  paused (frozen view must not advance or prune; callers that rebuild
   *  the engine reset the cursor explicitly). */
  const applyStatus = useCallback((reply: StatusMsg) => {
    setState(reply.state);
    setLoadedPath(reply.projectPath);
    setDirty(reply.dirty);
    setDisabledNodes(reply.disabled ?? []);
    if (pausedRef.current) return;
    cursorRef.current = reply.nextSeq;
    setRows((prev) => prev.filter((r) => r.seq < reply.nextSeq));
  }, []);

  const refreshStatus = useCallback(async (api: CanLabApi) => {
    const reply = await api.request({ type: "GetStatus" });
    if (reply.type === "Status") applyStatus(reply);
    else if (reply.type === "Error") showError(reply.message);
  }, [applyStatus, showError]);

  /** Rebuild the canvas from the server project, keeping drag positions of
   *  surviving ids and pruning positions of deleted ones. */
  const refreshProject = useCallback(async (api: CanLabApi) => {
    const reply = await api.request({ type: "GetProject" });
    if (reply.type !== "Project") {
      if (reply.type === "Error" && reply.message !== "no project loaded") showError(reply.message);
      setBuses([]);
      setNodes([]);
      setMessages([]);
      setFaults([]);
      setDisabledNodes([]);
      setFlowNodes([]);
      setFlowEdges([]);
      setCanvasSel(null);
      return;
    }
    const p = reply.project;
    const pb = p.buses ?? [];
    const pn = p.nodes ?? [];
    setBuses(pb);
    setNodes(pn);
    setMessages(p.messages ?? []);
    setFaults(p.faults ?? []);
    const pos = positionsRef.current;
    setFlowNodes([
      ...pb.map((b, i) => ({
        id: `bus:${b.id}`,
        type: "default",
        position: pos.get(`bus:${b.id}`) ?? defaultPos("bus", i),
        data: { label: `CAN BUS ${b.id} · ${b.bitrate}` },
        style: BUS_STYLE,
      })),
      ...pn.map((n, i) => ({
        id: `node:${n.id}`,
        type: "default",
        position: pos.get(`node:${n.id}`) ?? defaultPos("node", i),
        data: { label: `${n.id} · ${n.device}` },
        style: NODE_STYLE,
      })),
    ]);
    setFlowEdges(pn.map((n) => ({
      id: `e:${n.id}->${n.can.bus}`,
      source: `node:${n.id}`,
      target: `bus:${n.can.bus}`,
      deletable: false,
      animated: true,
      style: { stroke: "#60a5fa" },
    })));
    const alive = new Set([...pb.map((b) => `bus:${b.id}`), ...pn.map((n) => `node:${n.id}`)]);
    for (const key of [...pos.keys()]) if (!alive.has(key)) pos.delete(key);
    setCanvasSel((sel) => (sel && alive.has(sel) ? sel : null));
  }, [showError]);

  // Remember drag positions so server refreshes don't snap nodes back.
  useEffect(() => {
    const pos = positionsRef.current;
    flowNodes.forEach((n) => pos.set(n.id, n.position));
  }, [flowNodes]);

  const pollEvents = useCallback(async () => {
    const api = apiRef.current;
    if (!api?.connected) return;
    if (pausedRef.current) return; // capture paused: freeze the cursor, resume picks up the backlog
    try {
      const reply = await api.request({ type: "GetEvents", sinceSeq: cursorRef.current });
      if (reply.type !== "Events") return;
      cursorRef.current = reply.nextSeq;
      const fresh = analyzerRows(reply.events as SeqEvent[]);
      if (fresh.length > 0) {
        setRows((prev) => [...prev, ...fresh].slice(-MAX_ROWS));
      }
    } catch {
      // poll failure surfaces on next user action; stay quiet to avoid spam
    }
  }, []);

  /** Poll the Renode job table (same quiet contract as events). */
  const refreshJobs = useCallback(async () => {
    const api = apiRef.current;
    if (!api?.connected) return;
    try {
      const reply = await api.request({ type: "ListRenodeJobs" });
      if (reply.type === "RenodeJobList") setJobs(reply.jobs);
    } catch {
      // stay quiet to avoid spam; Start/Import surface errors directly
    }
  }, []);

  useEffect(() => {
    if (!connected) return;
    const timer = setInterval(() => {
      void pollEvents();
      void refreshJobs();
    }, 500);
    return () => clearInterval(timer);
  }, [connected, pollEvents, refreshJobs]);

  const connect = useCallback(async () => {
    setError(null);
    const api = new CanLabApi();
    api.onClose = () => {
      setConnected(false);
      apiRef.current = null;
    };
    try {
      await api.connect(WS_URL);
      apiRef.current = api;
      setConnected(true);
      await refreshStatus(api);
      await refreshProject(api); // picks up a --project preload, if any
      await refreshJobs(); // picks up jobs started by other tabs
    } catch (e) {
      showError(e instanceof Error ? e.message : String(e));
    }
  }, [refreshJobs, refreshStatus, refreshProject, showError]);

  const runCmd = useCallback(async (label: string, msg: ClientMsg) => {
    const api = apiRef.current;
    if (!api) {
      showError("not connected — press Connect first");
      return;
    }
    setError(null);
    setSummary(null);
    try {
      const reply = await api.request(msg);
      const err = replyError(reply);
      if (err) {
        showError(`${label}: ${err}`);
        return;
      }
      if (reply.type === "RunSummary") {
        setSummary(`${reply.transmitted} transmitted, ${reply.received} received`);
      } else if (reply.type === "FaultInjected") {
        setSummary(`Fault injected: ${reply.error ?? "decoded clean"} — ${reply.receivers} receiver(s).`);
      } else if (reply.type === "Arbitrated") {
        const idHex = `0x${reply.winnerId.toString(16).toUpperCase()}`;
        const lost = reply.losers.length > 0 ? ` — lost: ${reply.losers.join(", ")}` : " (uncontested)";
        setSummary(`Arbitration: ${reply.winner} wins ${idHex}${lost} — ${reply.receivers} receiver(s).`);
      }
      // Fetch-then-adopt: Start/Inject/Step append events BEFORE the Status
      // reply is read, so poll with the pre-Status cursor first — adopting
      // the grown cursor first would skip exactly the frames just produced.
      await pollEvents();
      await refreshStatus(api);
    } catch (e) {
      showError(e instanceof Error ? e.message : String(e));
    }
  }, [pollEvents, refreshStatus, showError]);

  /** Topology/file edits: apply, adopt status, rebuild canvas. */
  const editCmd = useCallback(async (label: string, msg: ClientMsg) => {
    const api = apiRef.current;
    if (!api) {
      showError("not connected — press Connect first");
      return;
    }
    setError(null);
    setSummary(null);
    try {
      const reply = await api.request(msg);
      const err = replyError(reply);
      if (err) {
        showError(`${label}: ${err}`);
        return;
      }
      // Every edit rebuilds the engine, so the log restarts: drop ALL rows,
      // not just rows past the cursor — survivors with low seq numbers would
      // belong to the previous topology (see bugs.md). The cursor resets
      // even when capture is paused (applyStatus freezes it then).
      if (reply.type === "Status") {
        applyStatus(reply);
        cursorRef.current = reply.nextSeq;
      }
      setRows([]);
      setSelected(null);
      setBitLayout(null);
      setDecoded(null);
      await refreshProject(api);
    } catch (e) {
      showError(e instanceof Error ? e.message : String(e));
    }
  }, [applyStatus, refreshProject, showError]);

  const load = useCallback(() => editCmd("Load", { type: "Load", path: projectPath }), [editCmd, projectPath]);

  /** Renode jobs: Start returns immediately (background thread on the
   *  server); TraceImported replays firmware traffic into the analyzer. */
  const renodeCmd = useCallback(async (label: string, msg: ClientMsg) => {
    const api = apiRef.current;
    if (!api) {
      showError("not connected — press Connect first");
      return;
    }
    setError(null);
    setSummary(null);
    try {
      const reply = await api.request(msg);
      const err = replyError(reply);
      if (err) {
        showError(`${label}: ${err}`);
        return;
      }
      if (reply.type === "RenodeJobStarted") {
        setSummary(`Renode job ${reply.jobId} started — polling until it finishes.`);
      } else if (reply.type === "TraceImported") {
        setSummary(`Imported firmware traffic: ${reply.transmitted} transmitted, ${reply.received} received.`);
      } else if (reply.type === "RenodeJob") {
        setSummary(`Renode job ${reply.job.jobId} is ${reply.job.state}.`);
      }
      // Same fetch-then-adopt as runCmd: import appends before Status is read.
      await pollEvents();
      await refreshStatus(api);
      await refreshJobs();
    } catch (e) {
      showError(e instanceof Error ? e.message : String(e));
    }
  }, [pollEvents, refreshJobs, refreshStatus, showError]);

  const startRenode = useCallback(() => {
    if (!renodePath.trim()) {
      showError("StartRenodeRun: enter a renode project path first");
      return;
    }
    const secs = Number(runSecsText);
    if (!Number.isInteger(secs) || secs < 1) {
      showError(`StartRenodeRun: run time must be an integer >= 1 (got "${runSecsText}")`);
      return;
    }
    void renodeCmd("StartRenodeRun", { type: "StartRenodeRun", path: renodePath.trim(), runSecs: secs });
  }, [renodeCmd, renodePath, runSecsText, showError]);

  /** Toggle one job's firmware log (last 20 UART lines; cancelled jobs
   *  have none — partials are discarded at cancel time). */
  const loadJobLog = useCallback(async (jobId: number) => {
    const api = apiRef.current;
    if (!api) {
      showError("not connected — press Connect first");
      return;
    }
    if (logJobId === jobId) {
      setLogJobId(null);
      return;
    }
    try {
      const reply = await api.request({ type: "GetRenodeLog", jobId, lastN: 20 });
      if (reply.type === "RenodeLog") {
        setLogJobId(jobId);
        setLogLines(reply.lines);
        setLogTotal(reply.total);
      } else if (reply.type === "Error") {
        showError(`GetRenodeLog: ${reply.message}`);
      }
    } catch (e) {
      showError(e instanceof Error ? e.message : String(e));
    }
  }, [logJobId, showError]);

  const newProject = useCallback(() => {
    if (!window.confirm("Discard the current session and start a blank project?")) return;
    void editCmd("New", { type: "NewProject" });
  }, [editCmd]);

  const save = useCallback(() => {
    if (!projectPath.trim()) {
      showError("Save: enter a file path first (save-as)");
      return;
    }
    void editCmd("Save", { type: "SaveProject", path: projectPath.trim() });
  }, [editCmd, projectPath, showError]);

  const addBus = useCallback(() => {
    const taken = buses.map((b) => b.id);
    void editCmd("AddBus", { type: "AddBus", id: nextId("bus", taken), bitrate: 500_000 });
  }, [buses, editCmd]);

  const addNode = useCallback((device: string) => {
    if (buses.length === 0) {
      showError("AddNode: add a CAN bus first, then drop nodes onto it");
      return;
    }
    const decl: NodeDecl = {
      id: nextId("node", nodes.map((n) => n.id)),
      device,
      backend: "virtual",
      can: { bus: buses[0].id },
    };
    void editCmd("AddNode", { type: "AddNode", node: decl });
  }, [buses, editCmd, nodes, showError]);

  const removeFlowNode = useCallback((flowId: string) => {
    const parsed = parseFlowId(flowId);
    if (!parsed) return;
    if (parsed.kind === "bus") void editCmd("RemoveBus", { type: "RemoveBus", id: parsed.name });
    else void editCmd("RemoveNode", { type: "RemoveNode", id: parsed.name });
  }, [editCmd]);

  /** Duplicate a node under a fresh id (same device/bus/firmware). Scripted
   *  messages and fault policies still name the original — re-point them
   *  explicitly if the copy should send instead. */
  const duplicateFlowNode = useCallback((flowId: string) => {
    const parsed = parseFlowId(flowId);
    if (!parsed || parsed.kind !== "node") {
      showError("Duplicate: select a node first (buses duplicate by Add, not copy)");
      return;
    }
    const decl = nodes.find((n) => n.id === parsed.name);
    if (!decl) {
      showError(`Duplicate: unknown node "${parsed.name}"`);
      return;
    }
    const next: NodeDecl = {
      id: nextId("node", nodes.map((n) => n.id)),
      device: decl.device,
      backend: decl.backend,
      firmware: decl.firmware ?? null,
      can: { bus: decl.can.bus },
      peripherals: decl.peripherals ?? [],
    };
    void editCmd("AddNode", { type: "AddNode", node: next });
  }, [editCmd, nodes, showError]);

  const onNodesChange = useCallback((changes: NodeChange[]) => {
    const removed = changes.filter((c) => c.type === "remove");
    if (removed.length > 0) {
      removed.forEach((c) => {
        if (c.type === "remove") removeFlowNode(c.id);
      });
    }
    // Removal is server-driven: don't apply it locally, wait for refresh.
    setFlowNodes((nds) => applyNodeChanges(changes.filter((c) => c.type !== "remove"), nds));
  }, [removeFlowNode]);

  /** Dragging an edge from a node onto a bus re-attaches it. */
  const onConnect = useCallback((conn: Connection) => {
    if (!conn.source || !conn.target) return;
    const a = parseFlowId(conn.source);
    const b = parseFlowId(conn.target);
    if (!a || !b || a.kind === b.kind) {
      showError("Connect: drag from a node onto a bus to attach it");
      return;
    }
    const nodeName = a.kind === "node" ? a.name : b.name;
    const busName = a.kind === "bus" ? a.name : b.name;
    const decl = nodes.find((n) => n.id === nodeName);
    if (!decl) {
      showError(`Connect: unknown node "${nodeName}"`);
      return;
    }
    const next: NodeDecl = {
      id: decl.id,
      device: decl.device,
      backend: decl.backend,
      firmware: decl.firmware ?? null,
      can: { bus: busName },
      peripherals: decl.peripherals ?? [],
    };
    void editCmd("Attach", { type: "UpdateNode", id: nodeName, node: next });
  }, [editCmd, nodes, showError]);

  const visibleRows = React.useMemo(() => {
    const f = filter.trim().toLowerCase();
    const spec = parseIdFilter(idFilterText);
    const kept = rows.filter((r) =>
      (dirFilter === "all" || r.dir === dirFilter) &&
      (!f || r.id.toLowerCase().includes(f) || r.node.toLowerCase().includes(f)) &&
      matchIdFilter(Number(r.id), spec));
    return newestFirst ? [...kept].reverse() : kept;
  }, [rows, filter, idFilterText, dirFilter, newestFirst]);
  const idFilterSpec = React.useMemo(() => parseIdFilter(idFilterText), [idFilterText]);

  const toggleCapture = useCallback(() => {
    setPaused((p) => {
      pausedRef.current = !p;
      return !p;
    });
  }, []);

  const clearAnalyzer = useCallback(() => {
    // View-local only: drops rendered rows, keeps the poll cursor. The
    // server log is untouched; already-consumed rows won't reappear, but
    // every new event still streams in.
    setRows([]);
    setSelected(null);
    setBitLayout(null);
    setDecoded(null);
  }, []);

  const exportCsv = useCallback(() => {
    const csv = ["seq,time_ms,dir,node,id,dlc,data", ...rows.map((r) => [r.seq, (r.timeNs / 1e6).toFixed(3), r.dir, r.node, r.id, r.dlc, `"${r.data}"`].join(","))].join("\n");
    const url = URL.createObjectURL(new Blob([csv], { type: "text/csv" }));
    const a = document.createElement("a");
    a.href = url;
    a.download = "canlab-trace.csv";
    a.click();
    URL.revokeObjectURL(url);
  }, [rows]);

  const exportJson = useCallback(() => {
    const payload = rows.map((r) => ({ seq: r.seq, timeMs: r.timeNs / 1e6, dir: r.dir, node: r.node, id: r.id, dlc: r.dlc, data: r.data, remote: r.remote }));
    const url = URL.createObjectURL(new Blob([JSON.stringify(payload, null, 2)], { type: "application/json" }));
    const a = document.createElement("a");
    a.href = url;
    a.download = "canlab-trace.json";
    a.click();
    URL.revokeObjectURL(url);
  }, [rows]);

  const exportPcap = useCallback(() => {
    const bytes = rowsToPcap(rows);
    const url = URL.createObjectURL(new Blob([bytes], { type: "application/vnd.tcpdump.pcap" }));
    const a = document.createElement("a");
    a.href = url;
    a.download = "canlab-trace.pcap";
    a.click();
    URL.revokeObjectURL(url);
  }, [rows]);

  /** Bit-level layout of the selected frame (§29): stateless InspectFrame
   *  round trip, rendered as a region table below the inspector. */
  const inspectBits = useCallback(async (row: AnalyzerRow) => {
    const api = apiRef.current;
    if (!api) {
      showError("not connected — press Connect first");
      return;
    }
    setError(null);
    try {
      const data = row.data.trim() === "" ? [] : row.data.trim().split(/\s+/).map((b) => Number(`0x${b}`));
      const reply = await api.request({
        type: "InspectFrame",
        id: Number(row.id),
        extended: row.extended,
        data,
        remote: row.remote,
        dlc: row.dlc,
      });
      if (reply.type === "FrameBits") setBitLayout(reply);
      else if (reply.type === "Error") showError(`InspectFrame: ${reply.message}`);
    } catch (e) {
      showError(e instanceof Error ? e.message : String(e));
    }
  }, [showError]);

  /** DBC signal decoding of the selected frame (§38): stateless
   *  DecodeFrame round trip against a server-side .dbc path. */
  const decodeSignals = useCallback(async (row: AnalyzerRow) => {
    const api = apiRef.current;
    if (!api) {
      showError("not connected — press Connect first");
      return;
    }
    if (!dbcPath.trim()) {
      showError("DecodeFrame: enter a DBC file path first (server-side, relative to serve cwd)");
      return;
    }
    setError(null);
    try {
      const data = row.data.trim() === "" ? [] : row.data.trim().split(/\s+/).map((b) => Number(`0x${b}`));
      const reply = await api.request({
        type: "DecodeFrame",
        dbc: dbcPath.trim(),
        id: Number(row.id),
        data,
      });
      if (reply.type === "DecodedSignals") {
        setDecoded({ message: reply.message, signals: reply.signals });
      } else if (reply.type === "Error") {
        showError(`DecodeFrame: ${reply.message}`);
      }
    } catch (e) {
      showError(e instanceof Error ? e.message : String(e));
    }
  }, [dbcPath, showError]);

  const btn: React.CSSProperties = { padding: "6px 12px", borderRadius: 6, border: "1px solid #374151", background: "#1f2937", color: "#e5e7eb", cursor: "pointer" };
  const selDecl = canvasSel ? parseFlowId(canvasSel) : null;
  const selNode = selDecl?.kind === "node" ? nodes.find((n) => n.id === selDecl.name) : undefined;
  const selBus = selDecl?.kind === "bus" ? buses.find((b) => b.id === selDecl.name) : undefined;

  return (
    <main style={{ fontFamily: "system-ui, sans-serif", background: "#030712", color: "#e5e7eb", minHeight: "100vh", padding: 16 }}>
      <header style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap", marginBottom: 12 }}>
        <h1 style={{ margin: 0, fontSize: 20 }}>CanLab</h1>
        <span aria-label={`simulation ${state}`} style={{ padding: "2px 10px", borderRadius: 999, background: state === "running" ? "#14532d" : "#1f2937", border: "1px solid #374151" }}>
          ● {state}
        </span>
        {connected && dirty && (
          <span aria-label="unsaved changes" style={{ padding: "2px 10px", borderRadius: 999, background: "#713f12", border: "1px solid #a16207" }}>
            ● unsaved
          </span>
        )}
        {connected && disabledNodes.length > 0 && (
          <span aria-label={`${disabledNodes.length} nodes disabled`} title={`Disabled: ${disabledNodes.join(", ")} (reset re-enables)`} style={{ padding: "2px 10px", borderRadius: 999, background: "#7f1d1d", border: "1px solid #ef4444" }}>
            ⛔ {disabledNodes.length} disabled
          </span>
        )}
        {!connected ? (
          <button style={btn} onClick={connect}>Connect</button>
        ) : (
          <>
            <input aria-label="project file path" title="Load/Save path (server-side, relative to serve cwd)" style={{ ...btn, minWidth: 300, cursor: "text" }} value={projectPath} onChange={(e) => setProjectPath(e.target.value)} />
            <button style={btn} onClick={load}>Load</button>
            <button style={btn} onClick={newProject}>New</button>
            <button style={btn} onClick={save}>Save</button>
            <button style={btn} onClick={() => runCmd("Start", { type: "Start" })}>▶ Run</button>
            <button style={btn} onClick={() => runCmd("Pause", { type: "Pause" })}>⏸ Pause</button>
            <button style={btn} onClick={() => runCmd("Resume", { type: "Resume" })}>Resume</button>
            <button style={btn} onClick={() => runCmd("Stop", { type: "Stop" })}>⏹ Stop</button>
            <button style={btn} onClick={() => runCmd("Reset", { type: "Reset" })}>↻ Reset</button>
            <button style={btn} onClick={() => runCmd("Step", { type: "Step", deltaNs: 238_000 })}>⏭ Step</button>
            <button style={btn} title="Resolve one simultaneous round from the scripted messages: all transmit at once, lowest ID wins" onClick={() => {
              if (messages.length === 0) {
                showError("Arbitrate: add scripted messages first — they become the contenders");
                return;
              }
              void runCmd("Arbitrate", { type: "Arbitrate", frames: messages.map((m) => ({ sender: m.sender, id: m.id, extended: m.extended ?? false, data: m.data })) });
            }}>⚔ Arbitrate</button>
          </>
        )}
      </header>

      {error && (
        <div role="alert" style={{ position: "sticky", top: 0, zIndex: 10, background: "#7f1d1d", border: "1px solid #ef4444", borderRadius: 8, padding: 10, marginBottom: 12, whiteSpace: "pre-wrap", maxHeight: "30vh", overflow: "auto" }}>
          {error}
        </div>
      )}
      {summary && <p style={{ color: "#86efac" }}>Run finished: {summary}.</p>}
      {connected && flowNodes.length > 0 && (
        <p style={{ color: "#9ca3af" }}>Project: {loadedPath ?? "(unsaved — enter a path and press Save)"} · paths resolve on the server, relative to the <code>canlab serve</code> cwd.</p>
      )}

      {connected && (
        <section aria-label="component palette" style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap", marginBottom: 12 }}>
          <strong style={{ fontSize: 13 }}>Palette:</strong>
          <button style={btn} onClick={addBus}>＋ CAN bus</button>
          {KNOWN_DEVICES.map((d) => (
            <button key={d} style={btn} onClick={() => addNode(d)}>＋ {d}</button>
          ))}
          <label style={{ fontSize: 13 }}>
            <input type="checkbox" checked={snapGrid} onChange={(e) => setSnapGrid(e.target.checked)} /> snap to grid
          </label>
        </section>
      )}

      <div style={{ display: "grid", gridTemplateColumns: "3fr 1fr", gap: 12, marginBottom: 12 }}>
        <section aria-label="network canvas" style={{ height: 380, background: "#0b1220", borderRadius: 12, border: "1px solid #1f2937" }}>
          {flowNodes.length === 0 ? (
            <p style={{ padding: 16, color: "#9ca3af" }}>
              {connected
                ? "Blank canvas — add a CAN bus from the palette, then drop nodes onto it. Or Load a project file."
                : "Press Connect, then Load a project — or start blank with New and build from the palette."}
            </p>
          ) : (
            <ReactFlow
              nodes={flowNodes}
              edges={flowEdges}
              onNodesChange={onNodesChange}
              onConnect={onConnect}
              onNodeClick={(_, n) => setCanvasSel(n.id)}
              onPaneClick={() => setCanvasSel(null)}
              deleteKeyCode={["Backspace", "Delete"]}
              snapToGrid={snapGrid}
              snapGrid={[16, 16]}
              fitView
            >
              <Background />
              <Controls />
            </ReactFlow>
          )}
        </section>

        <aside aria-label="properties" style={{ background: "#0b1220", borderRadius: 12, border: "1px solid #1f2937", padding: 12, fontSize: 13 }}>
          <h2 style={{ margin: "0 0 8px", fontSize: 16 }}>Properties</h2>
          {selNode ? (
            <NodeProps
              key={selNode.id}
              node={selNode}
              buses={buses}
              disabled={disabledNodes.includes(selNode.id)}
              onApply={(next) => editCmd("Update", { type: "UpdateNode", id: selNode.id, node: next })}
              onDelete={() => removeFlowNode(`node:${selNode.id}`)}
              onDuplicate={() => duplicateFlowNode(`node:${selNode.id}`)}
              onToggleEnabled={() => {
                const id = selNode.id;
                if (disabledNodes.includes(id)) void runCmd("EnableNode", { type: "EnableNode", id });
                else void runCmd("DisableNode", { type: "DisableNode", id });
              }}
            />
          ) : selBus ? (
            <BusProps
              key={selBus.id}
              bus={selBus}
              onApply={(bitrate) => editCmd("Update", { type: "UpdateBus", id: selBus.id, bitrate })}
              onDelete={() => removeFlowNode(`bus:${selBus.id}`)}
            />
          ) : (
            <p style={{ color: "#9ca3af" }}>Click a bus or node to edit it. Drag nodes to arrange; drag an edge onto another bus to re-attach. Delete key removes the selected node.</p>
          )}
        </aside>
      </div>

      {connected && flowNodes.length > 0 && (
        <MessagesPanel
          key={`messages:${nodes.map((n) => n.id).join(",")}`}
          messages={messages}
          nodes={nodes}
          onAdd={(message) => editCmd("AddMessage", { type: "AddMessage", message })}
          onUpdate={(index, message) => editCmd("UpdateMessage", { type: "UpdateMessage", index, message })}
          onRemove={(index) => editCmd("RemoveMessage", { type: "RemoveMessage", index })}
        />
      )}

      {connected && flowNodes.length > 0 && (
        <SketchImportPanel
          key={`sketch:${nodes.map((n) => n.id).join(",")}`}
          nodes={nodes}
          onAdd={(message) => editCmd("AddMessage", { type: "AddMessage", message })}
          onPreview={async (filename, content, dialect) => {
            const api = apiRef.current;
            if (!api) {
              showError("not connected — press Connect first");
              return null;
            }
            try {
              const reply = await api.request({ type: "ImportSketch", filename, content, dialect });
              if (reply.type === "SketchPreview") {
                return { detected: reply.detected, sends: reply.sends };
              }
              showError(reply.type === "Error" ? `ImportSketch: ${reply.message}` : "ImportSketch: unexpected reply");
              return null;
            } catch (e) {
              showError(e instanceof Error ? e.message : String(e));
              return null;
            }
          }}
        />
      )}

      {connected && flowNodes.length > 0 && (
        <FaultPoliciesPanel
          key={`faultpolicies:${nodes.map((n) => n.id).join(",")}`}
          faults={faults}
          nodes={nodes}
          onAdd={(fault) => editCmd("AddFault", { type: "AddFault", fault })}
          onRemove={(index) => editCmd("RemoveFault", { type: "RemoveFault", index })}
        />
      )}

      {connected && (
        <section aria-label="renode runs" style={{ background: "#0b1220", borderRadius: 12, border: "1px solid #1f2937", padding: 12, marginBottom: 12 }}>
          <h2 style={{ margin: "0 0 8px", fontSize: 16 }}>Renode firmware runs ({jobs.length})</h2>
          <p style={{ margin: "0 0 8px", color: "#9ca3af", fontSize: 13 }}>
            Real STM32F103 firmware in the background: Start returns immediately, the table polls until the
            job is done, then Import replays its observed TX frames into the analyzer. Full UART logs stay
            on the server — use <code>canlab simulate</code> for those.
          </p>
          <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap", marginBottom: 8 }}>
            <input aria-label="renode project file path" title="Renode project path (server-side, relative to serve cwd; all nodes must use backend renode)" style={{ ...btn, minWidth: 300, cursor: "text" }} value={renodePath} onChange={(e) => setRenodePath(e.target.value)} />
            <input aria-label="run time in seconds" title="wall-clock run budget in seconds" style={{ ...btn, width: 80, cursor: "text" }} value={runSecsText} onChange={(e) => setRunSecsText(e.target.value)} />
            <button style={btn} onClick={startRenode}>▶ Start firmware run</button>
          </div>
          {jobs.length > 0 && (
            <table style={{ width: "100%", borderCollapse: "collapse", fontSize: 13 }}>
              <thead>
                <tr style={{ textAlign: "left", color: "#9ca3af" }}>
                  <th>ID</th><th>Project</th><th>State</th><th>TX</th><th>RX</th><th>Error</th><th></th>
                </tr>
              </thead>
              <tbody>
                {jobs.map((j) => (
                  <tr key={j.jobId}>
                    <td>{j.jobId}</td>
                    <td style={{ fontFamily: "monospace" }}>{j.project}</td>
                    <td>
                      <span style={{ padding: "2px 10px", borderRadius: 999, background: j.state === "done" ? "#14532d" : j.state === "failed" ? "#7f1d1d" : j.state === "cancelled" ? "#713f12" : "#1f2937", border: "1px solid #374151" }}>
                        ● {j.state}
                      </span>
                    </td>
                    <td>{j.transmitted}</td>
                    <td>{j.received}</td>
                    <td style={{ color: "#fca5a5", maxWidth: 320, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }} title={j.error ?? ""}>{j.error ?? ""}</td>
                    <td style={{ whiteSpace: "nowrap" }}>
                      {j.state === "done" && <button style={btn} onClick={() => renodeCmd("ImportRenodeTrace", { type: "ImportRenodeTrace", jobId: j.jobId })}>Import trace</button>}
                      {j.state === "running" && <button style={btn} onClick={() => renodeCmd("CancelRenodeJob", { type: "CancelRenodeJob", jobId: j.jobId })}>Cancel</button>}
                      {(j.state === "done" || j.state === "failed" || j.state === "running") && <button style={btn} onClick={() => loadJobLog(j.jobId)}>{logJobId === j.jobId ? "Hide log" : "Log"}</button>}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          {logJobId !== null && (
            <pre aria-label="firmware log" style={{ background: "#111827", borderRadius: 8, padding: 10, fontSize: 12, maxHeight: 220, overflow: "auto", whiteSpace: "pre-wrap" }}>
              {`job ${logJobId} firmware log (last ${logLines.length} of ${logTotal} lines):\n` +
                (logLines.length > 0
                  ? logLines.map((l) => `[${l.machine}] ${l.message}`).join("\n")
                  : "(no firmware output captured)")}
            </pre>
          )}
        </section>
      )}

      {connected && nodes.length > 0 && (
        <FaultPanel
          key={`fault:${nodes.map((n) => n.id).join(",")}`}
          nodes={nodes}
          onFault={(sender, id, data, extended, fault) =>
            runCmd("InjectFault", { type: "InjectFault", sender, id, data, extended, fault })}
        />
      )}

      <section aria-label="can analyzer" style={{ background: "#0b1220", borderRadius: 12, border: "1px solid #1f2937", padding: 12 }}>
        <div style={{ display: "flex", gap: 8, alignItems: "center", marginBottom: 8, flexWrap: "wrap" }}>
          <h2 style={{ margin: 0, fontSize: 16 }}>CAN Analyzer</h2>
          {paused && <span aria-label="capture paused" style={{ padding: "2px 10px", borderRadius: 999, background: "#713f12", border: "1px solid #a16207" }}>⏸ paused</span>}
          <input aria-label="filter by id or node" placeholder="filter: id or node" style={{ ...btn, cursor: "text" }} value={filter} onChange={(e) => setFilter(e.target.value)} />
          <input aria-label="id filter" title="exact (0x123), range (0x100-0x2FF), or id/mask (0x120/0x7F0)" placeholder="id: 0x123, range, mask" style={{ ...btn, cursor: "text", width: 180 }} value={idFilterText} onChange={(e) => setIdFilterText(e.target.value)} />
          <button style={btn} onClick={() => setNewestFirst((v) => !v)}>{newestFirst ? "⇅ Oldest first" : "⇅ Newest first"}</button>
          <select aria-label="direction filter" title="TX/RX/DROP/ARB direction filter" style={btn} value={dirFilter} onChange={(e) => setDirFilter(e.target.value as "all" | "TX" | "RX" | "DROP" | "ARB")}>
            <option value="all">All</option>
            <option value="TX">TX only</option>
            <option value="RX">RX only</option>
            <option value="DROP">DROP only</option>
            <option value="ARB">ARB only</option>
          </select>
          <button style={btn} onClick={toggleCapture}>{paused ? "Resume capture" : "Pause capture"}</button>
          <button style={btn} onClick={clearAnalyzer}>Clear</button>
          <button style={btn} onClick={exportCsv}>Export CSV</button>
          <button style={btn} onClick={exportJson}>Export JSON</button>
          <button style={btn} onClick={exportPcap}>Export PCAP</button>
          <span style={{ color: "#9ca3af" }}>{visibleRows.length} row(s){rows.length >= MAX_ROWS ? ` (capped at ${MAX_ROWS})` : ""}</span>
          {idFilterSpec.kind === "invalid" && <span role="alert" style={{ color: "#fca5a5" }}>{idFilterSpec.message}</span>}
        </div>
        <div style={{ display: "grid", gridTemplateColumns: "2fr 1fr", gap: 12 }}>
          <table style={{ width: "100%", borderCollapse: "collapse", fontSize: 13 }}>
            <thead>
              <tr style={{ textAlign: "left", color: "#9ca3af" }}>
                <th>Time</th><th>Dir</th><th>Node</th><th>ID</th><th>DLC</th><th>Data</th>
              </tr>
            </thead>
            <tbody>
              {visibleRows.map((r) => (
                <tr key={r.seq} onClick={() => { setSelected(r); setBitLayout(null); setDecoded(null); }} style={{ cursor: "pointer", background: selected?.seq === r.seq ? "#1e3a8a" : "transparent" }}>
                  <td>{fmtTimeNs(r.timeNs)}</td>
                  <td>{r.dir}</td>
                  <td>{r.node}</td>
                  <td>{r.id}</td>
                  <td>{r.dlc}</td>
                  <td style={{ fontFamily: "monospace" }}>{r.data}</td>
                </tr>
              ))}
            </tbody>
          </table>
          <aside aria-label="frame inspector" style={{ border: "1px solid #1f2937", borderRadius: 8, padding: 10, minHeight: 120 }}>
            <h3 style={{ margin: "0 0 8px", fontSize: 14 }}>Frame inspector</h3>
            {selected ? (
              <>
              <dl style={{ margin: 0, fontSize: 13 }}>
                <dt>ID</dt><dd style={{ fontFamily: "monospace" }}>{selected.id}{selected.extended ? " (extended)" : ""}</dd>
                <dt>Direction</dt><dd>{selected.dir}</dd>
                <dt>Node</dt><dd>{selected.node}</dd>
                <dt>DLC</dt><dd>{selected.dlc}</dd>
                <dt>Frame</dt><dd>{selected.remote ? "Remote (RTR, no payload)" : "Data"}</dd>
                <dt>Data</dt><dd style={{ fontFamily: "monospace" }}>{selected.data || "(empty)"}</dd>
                <dt>Timestamp</dt><dd>{fmtTimeNs(selected.timeNs)}</dd>
              </dl>
              <button style={{ ...btn, marginTop: 8 }} onClick={() => inspectBits(selected)}>Show bit layout</button>
              <div style={{ display: "flex", gap: 8, alignItems: "center", marginTop: 8, flexWrap: "wrap" }}>
                <input aria-label="dbc file path" title="DBC file path (server-side, relative to serve cwd)" placeholder="examples/vehicle.dbc" style={{ ...btn, cursor: "text", minWidth: 200 }} value={dbcPath} onChange={(e) => setDbcPath(e.target.value)} />
                <button style={btn} onClick={() => decodeSignals(selected)}>Decode with DBC</button>
              </div>
              {decoded && (
                <div style={{ marginTop: 8, fontSize: 12 }}>
                  <p style={{ margin: "0 0 4px", color: "#9ca3af" }}>
                    {decoded.message} ({decoded.signals.length} signal(s)):
                  </p>
                  {decoded.signals.length > 0 ? (
                    <table style={{ width: "100%", borderCollapse: "collapse" }}>
                      <tbody>
                        {decoded.signals.map((s) => (
                          <tr key={s.name}>
                            <td>{s.name}</td>
                            <td style={{ fontFamily: "monospace" }}>{s.value}{s.unit ? ` ${s.unit}` : ""}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  ) : (
                    <p style={{ color: "#9ca3af" }}>(no signals defined for this message)</p>
                  )}
                </div>
              )}
              {bitLayout && (
                <div style={{ marginTop: 8, fontSize: 12 }}>
                  <p style={{ margin: "0 0 4px", color: "#9ca3af" }}>
                    {bitLayout.wireBits} wire bits · {bitLayout.stuffBits} stuff bits · CRC {bitLayout.crcHex}
                  </p>
                  <WaveformStrip wire={bitLayout.wire} />
                  <table style={{ width: "100%", borderCollapse: "collapse", marginTop: 4 }}>
                    <thead>
                      <tr style={{ textAlign: "left", color: "#9ca3af" }}>
                        <th>Field</th><th>Bits</th>
                      </tr>
                    </thead>
                    <tbody>
                      {bitLayout.regions.map((g) => (
                        <tr key={g.name}>
                          <td>{g.name}</td>
                          <td style={{ fontFamily: "monospace", wordBreak: "break-all" }}>{g.bits || "—"}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
              </>
            ) : (
              <p style={{ color: "#9ca3af" }}>Click a frame to inspect it.</p>
            )}
          </aside>
        </div>
      </section>
    </main>
  );
}

/** Waveform strip of one transmitted wire (Phase 10, slice 1): one cell
 *  per bit, dominant low + bright, recessive high + dim, with SOF and
 *  fixed-tail band tints. Pure render of the wire string — no interaction,
 *  no eyes-required layout (horizontal scroll for long frames). */
function WaveformStrip({ wire }: { wire: string }): JSX.Element {
  const bands = waveformBands(wire.length);
  const tail = bands.find((b) => b.name === "Tail");
  const H = 18;
  return (
    <div>
      <svg viewBox={`0 0 ${wire.length} ${H}`} style={{ width: "100%", height: 44 }} role="img" aria-label={`CAN wire waveform, ${wire.length} bits SOF to EOF`}>
        {tail && <rect x={tail.start} y={0} width={tail.len} height={H} fill="#713f12" opacity={0.45} />}
        <rect x={0} y={0} width={1} height={H} fill="#1d4ed8" opacity={0.45} />
        {wire.split("").map((b, i) => (
          <rect key={i} x={i + 0.05} y={b === "0" ? H / 2 : 0} width={0.9} height={H / 2} fill={b === "0" ? "#38bdf8" : "#334155"} />
        ))}
      </svg>
      <p style={{ margin: "2px 0 0", color: "#9ca3af", fontSize: 11 }}>
        ■ SOF&ensp;■ stuffed SOF..CRC&ensp;■ fixed tail (CRC delim + ACK + EOF)
      </p>
    </div>
  );
}

/** Arduino sketch import: pick a .ino/.cpp file, preview its CAN sends
 *  (dialect auto-detected from #includes, overridable), fill any dynamic
 *  cells with example bytes, and add rows as ordinary scripted messages
 *  with sketch provenance. Nothing is compiled or run. */
function SketchImportPanel({ nodes, onAdd, onPreview }: {
  nodes: ProjectNode[];
  onAdd: (message: MessageDecl) => void;
  onPreview: (filename: string, content: string, dialect: string | null) => Promise<{ detected: string[]; sends: SketchSend[] } | null>;
}): JSX.Element {
  const [dialect, setDialect] = useState("");
  const [preview, setPreview] = useState<{ filename: string; detected: string[]; sends: SketchSend[] } | null>(null);
  const [added, setAdded] = useState<Set<number>>(new Set());
  const [panelError, setPanelError] = useState<string | null>(null);
  const field: React.CSSProperties = { padding: 6, borderRadius: 6, border: "1px solid #374151", background: "#111827", color: "#e5e7eb" };
  const btn: React.CSSProperties = { padding: "6px 12px", borderRadius: 6, border: "1px solid #374151", background: "#1f2937", color: "#e5e7eb", cursor: "pointer" };

  const pickFile = (file: File | undefined) => {
    if (!file) return;
    setPanelError(null);
    setPreview(null);
    setAdded(new Set());
    const reader = new FileReader();
    reader.onload = () => {
      void (async () => {
        const result = await onPreview(file.name, String(reader.result ?? ""), dialect.trim() || null);
        if (result) setPreview({ filename: file.name, ...result });
        else setPanelError("preview failed — see the error above");
      })();
    };
    reader.onerror = () => setPanelError(`could not read ${file.name}`);
    reader.readAsText(file);
  };

  return (
    <section aria-label="sketch import" style={{ background: "#0b1220", borderRadius: 12, border: "1px solid #1f2937", padding: 12, marginBottom: 12 }}>
      <h2 style={{ margin: "0 0 8px", fontSize: 16 }}>Sketch import</h2>
      <p style={{ margin: "0 0 8px", color: "#9ca3af", fontSize: 13 }}>
        Static analysis only — the sketch is never compiled or run. Dynamic cells need example bytes before a row can be added.
      </p>
      {panelError && <p role="alert" style={{ color: "#fca5a5", margin: "0 0 8px" }}>{panelError}</p>}
      {preview && (
        <p style={{ margin: "0 0 8px", color: "#9ca3af", fontSize: 13 }}>
          {preview.filename}: detected {preview.detected.join(", ") || "(override)"} — {preview.sends.length} send(s).
        </p>
      )}
      <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
        <input aria-label="sketch file" title="Arduino sketch (.ino/.cpp) — content is sent as text, never executed" type="file" accept=".ino,.cpp,.h" style={field} onChange={(e) => pickFile(e.target.files?.[0])} />
        <input aria-label="dialect override" title="empty = auto-detect from #includes; or mcp_can / arduino-can" placeholder="dialect (auto)" style={{ ...field, width: 160 }} value={dialect} onChange={(e) => setDialect(e.target.value)} />
      </div>
      {preview && preview.sends.length > 0 && (
        <table style={{ width: "100%", borderCollapse: "collapse", fontSize: 13, marginTop: 8 }}>
          <thead>
            <tr style={{ textAlign: "left", color: "#9ca3af" }}>
              <th>Line</th><th>Lib</th><th>Sender</th><th>ID</th><th>Data (hex, ? = fill)</th><th></th>
            </tr>
          </thead>
          <tbody>
            {preview.sends.map((s, i) => (
              <SketchSendRow
                key={i}
                send={s}
                nodes={nodes}
                filename={preview.filename}
                added={added.has(i)}
                onAdd={(m) => {
                  onAdd(m);
                  setAdded((prev) => new Set(prev).add(i));
                }}
              />
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}

/** One previewed send: constant cells render static, dynamic cells get
 *  fill inputs. Add validates everything (hex id/range, hex bytes, ≤8)
 *  before the row becomes an ordinary scripted message with provenance. */
function SketchSendRow({ send, nodes, filename, added, onAdd }: {
  send: SketchSend;
  nodes: ProjectNode[];
  filename: string;
  added: boolean;
  onAdd: (message: MessageDecl) => void;
}): JSX.Element {
  const [sender, setSender] = useState(nodes[0]?.id ?? "");
  const [idText, setIdText] = useState("Const" in send.id ? `0x${send.id.Const.toString(16).toUpperCase()}` : "");
  const [byteTexts, setByteTexts] = useState<string[]>(
    send.data.map((b) => ("Const" in b ? b.Const.toString(16).toUpperCase().padStart(2, "0") : ""))
  );
  const [extended, setExtended] = useState("Const" in send.extended ? send.extended.Const : false);
  const [formError, setFormError] = useState<string | null>(null);
  const field: React.CSSProperties = { padding: 6, borderRadius: 6, border: "1px solid #374151", background: "#111827", color: "#e5e7eb" };
  const btn: React.CSSProperties = { padding: "6px 12px", borderRadius: 6, border: "1px solid #374151", background: "#1f2937", color: "#e5e7eb", cursor: "pointer" };

  const submit = () => {
    setFormError(null);
    if (!sender) {
      setFormError("pick a sending node first");
      return;
    }
    const id = Number(idText.trim().toLowerCase().startsWith("0x") ? idText.trim() : `0x${idText.trim()}`);
    if (!Number.isInteger(id) || id < 0) {
      setFormError(`"${idText}" is not a hex frame id (fill every dynamic cell first)`);
      return;
    }
    const max = extended ? 0x1fffffff : 0x7ff;
    if (id > max) {
      setFormError(`0x${id.toString(16).toUpperCase()} exceeds the ${extended ? "29-bit extended" : "11-bit standard"} range`);
      return;
    }
    const data: number[] = [];
    for (const text of byteTexts) {
      const b = Number(`0x${text.trim()}`);
      if (!Number.isInteger(b) || b < 0 || b > 0xff) {
        setFormError(`"${text}" is not a hex byte (00–FF)`);
        return;
      }
      data.push(b);
    }
    if (data.length > 8) {
      setFormError("Classical CAN carries at most 8 data bytes");
      return;
    }
    onAdd({ sender, id, data, extended, source: `${filename}:${send.line}` });
  };

  return (
    <tr style={{ opacity: added ? 0.55 : 1 }}>
      <td>{send.line}</td>
      <td style={{ fontFamily: "monospace" }}>{send.library}</td>
      <td>
        <select aria-label="sending node" style={field} value={sender} onChange={(e) => setSender(e.target.value)}>
          {nodes.length === 0 && <option value="">(add a node first)</option>}
          {nodes.map((n) => <option key={n.id} value={n.id}>{n.id}</option>)}
        </select>
      </td>
      <td>
        <input aria-label="frame id in hex" style={{ ...field, width: 80 }} value={idText} onChange={(e) => setIdText(e.target.value)} />
      </td>
      <td>
        <span style={{ display: "inline-flex", gap: 4, flexWrap: "wrap" }}>
          {byteTexts.map((text, i) => (
            <input key={i} aria-label={`data byte ${i} in hex`} title={"Const" in send.data[i] ? "resolved constant (editable)" : `dynamic: ${JSON.stringify((send.data[i] as { Dynamic: string }).Dynamic)}`} style={{ ...field, width: 44, borderColor: "Const" in send.data[i] ? "#374151" : "#a16207" }} value={text} onChange={(e) => setByteTexts((prev) => prev.map((t, j) => (j === i ? e.target.value : t)))} />
          ))}
          {byteTexts.length === 0 && <span style={{ color: "#9ca3af" }}>(empty)</span>}
        </span>
      </td>
      <td style={{ whiteSpace: "nowrap" }}>
        <label style={{ fontSize: 12, marginRight: 8 }}>
          <input type="checkbox" checked={extended} onChange={(e) => setExtended(e.target.checked)} /> ext
        </label>
        <button style={btn} onClick={submit}>{added ? "✓ Added" : "Add"}</button>
      </td>
    </tr>
  );
}

/** Scripted traffic editor: what Run transmits, in order. Add form parses
 *  hex (`0x123` / `01 02`) and reports parse problems inline, before the
 *  server ever sees them; server-side validation is the backstop. */
function MessagesPanel({ messages, nodes, onAdd, onUpdate, onRemove }: {
  messages: MessageDecl[];
  nodes: ProjectNode[];
  onAdd: (message: MessageDecl) => void;
  onUpdate: (index: number, message: MessageDecl) => void;
  onRemove: (index: number) => void;
}): JSX.Element {
  const [sender, setSender] = useState(nodes[0]?.id ?? "");
  const [idText, setIdText] = useState("0x123");
  const [dataText, setDataText] = useState("01 02 03 04");
  const [extended, setExtended] = useState(false);
  const [editing, setEditing] = useState<number | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  const field: React.CSSProperties = { padding: 6, borderRadius: 6, border: "1px solid #374151", background: "#111827", color: "#e5e7eb" };
  const btn: React.CSSProperties = { padding: "6px 12px", borderRadius: 6, border: "1px solid #374151", background: "#1f2937", color: "#e5e7eb", cursor: "pointer" };

  /** Load a row into the form for in-place editing (UpdateMessage). */
  const beginEdit = (index: number) => {
    const m = messages[index];
    if (!m) return;
    setEditing(index);
    setSender(m.sender);
    setIdText(`0x${m.id.toString(16).toUpperCase()}`);
    setDataText(m.data.map((b) => b.toString(16).toUpperCase().padStart(2, "0")).join(" "));
    setExtended(m.extended ?? false);
    setFormError(null);
  };

  const cancelEdit = () => {
    setEditing(null);
    setFormError(null);
  };

  const submit = () => {
    setFormError(null);
    if (!sender) {
      setFormError("pick a sending node first");
      return;
    }
    const id = Number(idText.trim().toLowerCase().startsWith("0x") ? idText.trim() : `0x${idText.trim()}`);
    if (!Number.isInteger(id) || id < 0) {
      setFormError(`"${idText}" is not a hex frame id (e.g. 0x123)`);
      return;
    }
    const max = extended ? 0x1fffffff : 0x7ff;
    if (id > max) {
      setFormError(`0x${id.toString(16).toUpperCase()} exceeds the ${extended ? "29-bit extended" : "11-bit standard"} range`);
      return;
    }
    const parts = dataText.trim() === "" ? [] : dataText.trim().split(/[\s,]+/);
    const data: number[] = [];
    for (const p of parts) {
      const b = Number(`0x${p}`);
      if (!Number.isInteger(b) || b < 0 || b > 0xff) {
        setFormError(`"${p}" is not a hex byte (00–FF, space separated)`);
        return;
      }
      data.push(b);
    }
    if (data.length > 8) {
      setFormError("Classical CAN carries at most 8 data bytes");
      return;
    }
    const decl = { sender, id, data, extended };
    if (editing !== null) {
      onUpdate(editing, decl);
      setEditing(null);
    } else {
      onAdd(decl);
    }
  };

  return (
    <section aria-label="scripted messages" style={{ background: "#0b1220", borderRadius: 12, border: "1px solid #1f2937", padding: 12, marginBottom: 12 }}>
      <h2 style={{ margin: "0 0 8px", fontSize: 16 }}>Scripted traffic ({messages.length})</h2>
      {editing !== null && (
        <p style={{ margin: "0 0 8px", color: "#fbbf24", fontSize: 13 }}>
          Editing row {editing} — Update applies it, Cancel keeps the list unchanged.
        </p>
      )}
      <p style={{ margin: "0 0 8px", color: "#9ca3af", fontSize: 13 }}>
        Frames Run transmits in order. Deleting a row shifts later indices (see bugs.md); the list refreshes after every op.
      </p>
      {messages.length > 0 && (
        <table style={{ width: "100%", borderCollapse: "collapse", fontSize: 13, marginBottom: 8 }}>
          <thead>
            <tr style={{ textAlign: "left", color: "#9ca3af" }}>
              <th>#</th><th>Sender</th><th>ID</th><th>DLC</th><th>Data</th><th></th><th></th>
            </tr>
          </thead>
          <tbody>
            {messages.map((m, i) => (
              <tr key={i} style={{ background: editing === i ? "#1e3a8a" : "transparent" }}>
                <td>{i}</td>
                <td>{m.sender}{m.source ? <><br /><span title="imported from sketch" style={{ color: "#9ca3af", fontSize: 11 }}>⤴ {m.source}</span></> : null}</td>
                <td style={{ fontFamily: "monospace" }}>0x{m.id.toString(16).toUpperCase()}{m.extended ? " (ext)" : ""}</td>
                <td>{m.data.length}</td>
                <td style={{ fontFamily: "monospace" }}>{m.data.map((b) => b.toString(16).toUpperCase().padStart(2, "0")).join(" ") || "(empty)"}</td>
                <td><button style={btn} onClick={() => beginEdit(i)}>Edit</button></td>
                <td><button style={btn} onClick={() => { if (editing === i) setEditing(null); onRemove(i); }}>Delete</button></td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
        <select aria-label="sending node" style={field} value={sender} onChange={(e) => setSender(e.target.value)}>
          {nodes.length === 0 && <option value="">(add a node first)</option>}
          {nodes.map((n) => <option key={n.id} value={n.id}>{n.id}</option>)}
        </select>
        <input aria-label="frame id in hex" title="hex frame id, e.g. 0x123" style={{ ...field, width: 90 }} value={idText} onChange={(e) => setIdText(e.target.value)} />
        <input aria-label="data bytes in hex" title="space-separated hex bytes, e.g. 01 02 03 04" style={{ ...field, minWidth: 200 }} value={dataText} onChange={(e) => setDataText(e.target.value)} />
        <label style={{ fontSize: 13 }}>
          <input type="checkbox" checked={extended} onChange={(e) => setExtended(e.target.checked)} /> extended
        </label>
        <button style={btn} onClick={submit}>{editing !== null ? "Update frame" : "＋ Add frame"}</button>
        {editing !== null && <button style={btn} onClick={cancelEdit}>Cancel</button>}
      </div>
      {formError && <p role="alert" style={{ color: "#fca5a5", margin: "8px 0 0" }}>{formError}</p>}
    </section>
  );
}

/** Single-shot fault injection (Phase 9): drive one corrupted/dropped frame.
 *  Hex parsing mirrors MessagesPanel; the offset is a raw SOF..EOF bit
 *  index (server validates it against the stuffed wire length). CorruptCrc
 *  always surfaces a CRC error; DropFrame emits a DROP analyzer row. */
function FaultPanel({ nodes, onFault }: {
  nodes: ProjectNode[];
  onFault: (sender: string, id: number, data: number[], extended: boolean, fault: WireFault) => void;
}): JSX.Element {
  const [sender, setSender] = useState(nodes[0]?.id ?? "");
  const [idText, setIdText] = useState("0x123");
  const [dataText, setDataText] = useState("01 02 03 04");
  const [extended, setExtended] = useState(false);
  const [faultKind, setFaultKind] = useState<"flip" | "crc" | "drop">("crc");
  const [offsetText, setOffsetText] = useState("0");
  const [formError, setFormError] = useState<string | null>(null);
  const field: React.CSSProperties = { padding: 6, borderRadius: 6, border: "1px solid #374151", background: "#111827", color: "#e5e7eb" };
  const btn: React.CSSProperties = { padding: "6px 12px", borderRadius: 6, border: "1px solid #374151", background: "#1f2937", color: "#e5e7eb", cursor: "pointer" };

  const submit = () => {
    setFormError(null);
    if (!sender) {
      setFormError("pick a sending node first");
      return;
    }
    const id = Number(idText.trim().toLowerCase().startsWith("0x") ? idText.trim() : `0x${idText.trim()}`);
    if (!Number.isInteger(id) || id < 0) {
      setFormError(`"${idText}" is not a hex frame id (e.g. 0x123)`);
      return;
    }
    const max = extended ? 0x1fffffff : 0x7ff;
    if (id > max) {
      setFormError(`0x${id.toString(16).toUpperCase()} exceeds the ${extended ? "29-bit extended" : "11-bit standard"} range`);
      return;
    }
    const parts = dataText.trim() === "" ? [] : dataText.trim().split(/[\s,]+/);
    const data: number[] = [];
    for (const p of parts) {
      const b = Number(`0x${p}`);
      if (!Number.isInteger(b) || b < 0 || b > 0xff) {
        setFormError(`"${p}" is not a hex byte (00–FF, space separated)`);
        return;
      }
      data.push(b);
    }
    if (data.length > 8) {
      setFormError("Classical CAN carries at most 8 data bytes");
      return;
    }
    let fault: WireFault;
    if (faultKind === "flip") {
      const offset = Number(offsetText);
      if (!Number.isInteger(offset) || offset < 0) {
        setFormError(`"${offsetText}" is not a wire bit offset (integer >= 0)`);
        return;
      }
      fault = { FlipBit: offset };
    } else if (faultKind === "crc") {
      fault = "CorruptCrc";
    } else {
      fault = "DropFrame";
    }
    onFault(sender, id, data, extended, fault);
  };

  return (
    <section aria-label="fault injection" style={{ background: "#0b1220", borderRadius: 12, border: "1px solid #1f2937", padding: 12, marginBottom: 12 }}>
      <h2 style={{ margin: "0 0 8px", fontSize: 16 }}>Fault injection</h2>
      <p style={{ margin: "0 0 8px", color: "#9ca3af", fontSize: 13 }}>
        Drive one corrupted frame now (deterministic, single-shot). Dropped frames appear as DROP rows
        in the analyzer; error counters move on the bus. Probabilistic policies are deferred.
      </p>
      <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
        <select aria-label="sending node" style={field} value={sender} onChange={(e) => setSender(e.target.value)}>
          {nodes.map((n) => <option key={n.id} value={n.id}>{n.id}</option>)}
        </select>
        <input aria-label="frame id in hex" title="hex frame id, e.g. 0x123" style={{ ...field, width: 90 }} value={idText} onChange={(e) => setIdText(e.target.value)} />
        <input aria-label="data bytes in hex" title="space-separated hex bytes, e.g. 01 02 03 04" style={{ ...field, minWidth: 200 }} value={dataText} onChange={(e) => setDataText(e.target.value)} />
        <label style={{ fontSize: 13 }}>
          <input type="checkbox" checked={extended} onChange={(e) => setExtended(e.target.checked)} /> extended
        </label>
        <select aria-label="fault kind" title="FlipBit flips one SOF..EOF wire bit; CorruptCrc breaks the CRC; DropFrame drives nothing" style={field} value={faultKind} onChange={(e) => setFaultKind(e.target.value as "flip" | "crc" | "drop")}>
          <option value="crc">Corrupt CRC</option>
          <option value="flip">Flip wire bit</option>
          <option value="drop">Drop frame</option>
        </select>
        {faultKind === "flip" && (
          <input aria-label="wire bit offset" title="0-based SOF..EOF bit index (server validates against the wire length)" style={{ ...field, width: 90 }} value={offsetText} onChange={(e) => setOffsetText(e.target.value)} />
        )}
        <button style={btn} onClick={submit}>⚡ Inject fault</button>
      </div>
      {formError && <p role="alert" style={{ color: "#fca5a5", margin: "8px 0 0" }}>{formError}</p>}
    </section>
  );
}

/** One-line fault description for policy tables. */
function faultLabel(fault: WireFault): string {
  if (typeof fault === "string") return fault;
  return `FlipBit @${fault.FlipBit}`;
}

/** Project fault-policy editor: what the engine draws against on every
 *  Run, in order (first match wins). Add form validates ranges inline;
 *  server-side validation mirrors validate_project as the backstop.
 *  Removing a node still named here is refused until its rows go. */
function FaultPoliciesPanel({ faults, nodes, onAdd, onRemove }: {
  faults: FaultDecl[];
  nodes: ProjectNode[];
  onAdd: (fault: FaultDecl) => void;
  onRemove: (index: number) => void;
}): JSX.Element {
  const [nodeSel, setNodeSel] = useState("");
  const [faultKind, setFaultKind] = useState<"drop" | "crc" | "flip">("drop");
  const [offsetText, setOffsetText] = useState("0");
  const [idText, setIdText] = useState("");
  const [extended, setExtended] = useState(false);
  const [probText, setProbText] = useState("0.5");
  const [seedText, setSeedText] = useState("1");
  const [formError, setFormError] = useState<string | null>(null);
  const field: React.CSSProperties = { padding: 6, borderRadius: 6, border: "1px solid #374151", background: "#111827", color: "#e5e7eb" };
  const btn: React.CSSProperties = { padding: "6px 12px", borderRadius: 6, border: "1px solid #374151", background: "#1f2937", color: "#e5e7eb", cursor: "pointer" };

  const submit = () => {
    setFormError(null);
    let fault: WireFault;
    if (faultKind === "flip") {
      const offset = Number(offsetText);
      if (!Number.isInteger(offset) || offset < 0) {
        setFormError(`"${offsetText}" is not a wire bit offset (integer >= 0)`);
        return;
      }
      fault = { FlipBit: offset };
    } else if (faultKind === "crc") {
      fault = "CorruptCrc";
    } else {
      fault = "DropFrame";
    }
    let id: number | null = null;
    if (idText.trim() !== "") {
      const parsed = Number(idText.trim().toLowerCase().startsWith("0x") ? idText.trim() : `0x${idText.trim()}`);
      if (!Number.isInteger(parsed) || parsed < 0) {
        setFormError(`"${idText}" is not a hex frame id (e.g. 0x123, empty = all ids)`);
        return;
      }
      const max = extended ? 0x1fffffff : 0x7ff;
      if (parsed > max) {
        setFormError(`0x${parsed.toString(16).toUpperCase()} exceeds the ${extended ? "29-bit extended" : "11-bit standard"} range`);
        return;
      }
      id = parsed;
    }
    const probability = Number(probText);
    if (!Number.isFinite(probability) || probability < 0 || probability > 1) {
      setFormError(`"${probText}" is not a probability (0.0–1.0)`);
      return;
    }
    const seed = Number(seedText);
    if (!Number.isInteger(seed) || seed < 0 || seed > Number.MAX_SAFE_INTEGER) {
      setFormError(`"${seedText}" is not a seed (integer 0–2^53-1)`);
      return;
    }
    onAdd({ fault, node: nodeSel === "" ? null : nodeSel, id, extended, probability, seed });
  };

  return (
    <section aria-label="fault policies" style={{ background: "#0b1220", borderRadius: 12, border: "1px solid #1f2937", padding: 12, marginBottom: 12 }}>
      <h2 style={{ margin: "0 0 8px", fontSize: 16 }}>Fault policies ({faults.length})</h2>
      <p style={{ margin: "0 0 8px", color: "#9ca3af", fontSize: 13 }}>
        Seeded policies every Run draws against, in order (first match wins). Same project, same log.
        Deleting a row shifts later indices; a node named here cannot be removed first.
      </p>
      {faults.length > 0 && (
        <table style={{ width: "100%", borderCollapse: "collapse", fontSize: 13, marginBottom: 8 }}>
          <thead>
            <tr style={{ textAlign: "left", color: "#9ca3af" }}>
              <th>#</th><th>Fault</th><th>Node</th><th>ID</th><th>P</th><th>Seed</th><th></th>
            </tr>
          </thead>
          <tbody>
            {faults.map((f, i) => (
              <tr key={i}>
                <td>{i}</td>
                <td style={{ fontFamily: "monospace" }}>{faultLabel(f.fault)}</td>
                <td>{f.node ?? "(all)"}</td>
                <td style={{ fontFamily: "monospace" }}>{f.id === null || f.id === undefined ? "(all)" : `0x${f.id.toString(16).toUpperCase()}${f.extended ? " (ext)" : ""}`}</td>
                <td>{f.probability}</td>
                <td style={{ fontFamily: "monospace" }}>{f.seed}</td>
                <td><button style={btn} onClick={() => onRemove(i)}>Delete</button></td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
        <select aria-label="fault kind" style={field} value={faultKind} onChange={(e) => setFaultKind(e.target.value as "drop" | "crc" | "flip")}>
          <option value="drop">Drop frame</option>
          <option value="crc">Corrupt CRC</option>
          <option value="flip">Flip wire bit</option>
        </select>
        {faultKind === "flip" && (
          <input aria-label="wire bit offset" title="0-based SOF..EOF bit index" style={{ ...field, width: 90 }} value={offsetText} onChange={(e) => setOffsetText(e.target.value)} />
        )}
        <select aria-label="policy node" title="empty = all nodes" style={field} value={nodeSel} onChange={(e) => setNodeSel(e.target.value)}>
          <option value="">(all nodes)</option>
          {nodes.map((n) => <option key={n.id} value={n.id}>{n.id}</option>)}
        </select>
        <input aria-label="policy id in hex" title="hex frame id, e.g. 0x123 (empty = all ids)" placeholder="id (all)" style={{ ...field, width: 110 }} value={idText} onChange={(e) => setIdText(e.target.value)} />
        <label style={{ fontSize: 13 }}>
          <input type="checkbox" checked={extended} onChange={(e) => setExtended(e.target.checked)} /> extended
        </label>
        <input aria-label="probability" title="per-transmission probability 0.0–1.0" style={{ ...field, width: 70 }} value={probText} onChange={(e) => setProbText(e.target.value)} />
        <input aria-label="seed" title="draw-stream seed (integer)" style={{ ...field, width: 110 }} value={seedText} onChange={(e) => setSeedText(e.target.value)} />
        <button style={btn} onClick={submit}>＋ Add policy</button>
      </div>
      {formError && <p role="alert" style={{ color: "#fca5a5", margin: "8px 0 0" }}>{formError}</p>}
    </section>
  );
}

function NodeProps({ node, buses, disabled, onApply, onDelete, onDuplicate, onToggleEnabled }: {
  node: ProjectNode;
  buses: ProjectBus[];
  disabled: boolean;
  onApply: (next: NodeDecl) => void;
  onDelete: () => void;
  onDuplicate: () => void;
  onToggleEnabled: () => void;
}): JSX.Element {
  const [device, setDevice] = useState(node.device);
  const [firmware, setFirmware] = useState(node.firmware ?? "");
  const [bus, setBus] = useState(node.can.bus);
  const field: React.CSSProperties = { display: "block", width: "100%", boxSizing: "border-box", margin: "4px 0 10px", padding: 6, borderRadius: 6, border: "1px solid #374151", background: "#111827", color: "#e5e7eb" };
  const btn: React.CSSProperties = { padding: "6px 12px", borderRadius: 6, border: "1px solid #374151", background: "#1f2937", color: "#e5e7eb", cursor: "pointer", marginRight: 8 };
  return (
    <div>
      <p style={{ margin: "0 0 8px" }}><strong>{node.id}</strong> · backend virtual (live session runs virtual nodes only){disabled && " · ⛔ disabled"}</p>
      <label>Device
        <select style={field} value={device} onChange={(e) => setDevice(e.target.value)}>
          {KNOWN_DEVICES.map((d) => <option key={d} value={d}>{d}</option>)}
        </select>
      </label>
      <label>Bus
        <select style={field} value={bus} onChange={(e) => setBus(e.target.value)}>
          {buses.map((b) => <option key={b.id} value={b.id}>{b.id}</option>)}
        </select>
      </label>
      <label>Firmware path (optional, server-relative)
        <input style={field} value={firmware} placeholder="./firmware/node.bin" onChange={(e) => setFirmware(e.target.value)} />
      </label>
      <button style={btn} onClick={() => onApply({
        id: node.id,
        device,
        backend: "virtual",
        firmware: firmware.trim() ? firmware.trim() : null,
        can: { bus },
        peripherals: node.peripherals ?? [],
      })}>Apply</button>
      <button style={{ ...btn, borderColor: "#7f1d1d" }} onClick={onDelete}>Delete node</button>
      <button style={btn} title="Copy this node under a fresh id (messages/policies keep naming the original)" onClick={onDuplicate}>Duplicate node</button>
      <button style={btn} title="Runtime-only: disabled nodes neither drive nor receive; reset re-enables" onClick={onToggleEnabled}>{disabled ? "Enable node" : "Disable node"}</button>
    </div>
  );
}

function BusProps({ bus, onApply, onDelete }: {
  bus: ProjectBus;
  onApply: (bitrate: number) => void;
  onDelete: () => void;
}): JSX.Element {
  const [bitrate, setBitrate] = useState(String(bus.bitrate));
  const field: React.CSSProperties = { display: "block", width: "100%", boxSizing: "border-box", margin: "4px 0 10px", padding: 6, borderRadius: 6, border: "1px solid #374151", background: "#111827", color: "#e5e7eb" };
  const btn: React.CSSProperties = { padding: "6px 12px", borderRadius: 6, border: "1px solid #374151", background: "#1f2937", color: "#e5e7eb", cursor: "pointer", marginRight: 8 };
  return (
    <div>
      <p style={{ margin: "0 0 8px" }}><strong>{bus.id}</strong> · type can</p>
      <label>Bitrate (bit/s)
        <input style={field} inputMode="numeric" value={bitrate} onChange={(e) => setBitrate(e.target.value)} />
      </label>
      <button style={btn} onClick={() => {
        const n = Number(bitrate);
        if (!Number.isInteger(n) || n <= 0) {
          alert("Bitrate must be a positive integer");
          return;
        }
        onApply(n);
      }}>Apply</button>
      <button style={{ ...btn, borderColor: "#7f1d1d" }} onClick={onDelete}>Delete bus</button>
    </div>
  );
}
