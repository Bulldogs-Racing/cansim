import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import ReactFlow, { Background, Controls, Edge, Node } from "reactflow";
import "reactflow/dist/style.css";
import {
  AnalyzerRow,
  analyzerRows,
  CanLabApi,
  EngineState,
  fmtTimeNs,
  SeqEvent,
  ServerMsg,
} from "./api";

const MAX_ROWS = 500; // ring buffer (§67): UI never grows without bound
const WS_URL = "ws://127.0.0.1:21011";

interface ProjectNode {
  id: string;
  device: string;
  backend: string;
  firmware?: string;
  can: { bus: string };
}

interface ProjectBus {
  id: string;
  bitrate: number;
}

/** Layout: bus centered, MCU nodes fanned above/below it. */
function toFlowNodes(buses: ProjectBus[], nodes: ProjectNode[]): { flowNodes: Node[]; edges: Edge[] } {
  const flowNodes: Node[] = buses.map((b, i) => ({
    id: `bus:${b.id}`,
    type: "default",
    position: { x: 260, y: i * 220 + 120 },
    data: { label: `CAN BUS ${b.id}` },
    style: { background: "#1d4ed8", color: "#fff", borderRadius: 8, padding: 10, width: 220, textAlign: "center" },
  }));
  nodes.forEach((n, i) => {
    const side = i % 2 === 0 ? -1 : 1;
    const row = Math.floor(i / 2);
    flowNodes.push({
      id: `node:${n.id}`,
      type: "default",
      position: { x: 260 + side * 320, y: row * 160 + 40 },
      data: { label: `${n.id} · ${n.device}` },
      style: { background: "#111827", color: "#e5e7eb", border: "1px solid #374151", borderRadius: 8, padding: 10, width: 200, textAlign: "center" },
    });
  });
  const edges: Edge[] = nodes.map((n) => ({
    id: `e:${n.id}->${n.can.bus}`,
    source: `node:${n.id}`,
    target: `bus:${n.can.bus}`,
    animated: true,
    style: { stroke: "#60a5fa" },
  }));
  return { flowNodes, edges };
}

function replyError(reply: ServerMsg): string | null {
  return reply.type === "Error" ? reply.message : null;
}

export default function App(): JSX.Element {
  const apiRef = useRef<CanLabApi | null>(null);
  const cursorRef = useRef(0);
  const [connected, setConnected] = useState(false);
  const [state, setState] = useState<EngineState>("idle");
  const [projectPath, setProjectPath] = useState("examples/two_nodes.canlab.yaml");
  const [loadedPath, setLoadedPath] = useState<string | null>(null);
  const [buses, setBuses] = useState<ProjectBus[]>([]);
  const [nodes, setNodes] = useState<ProjectNode[]>([]);
  const [rows, setRows] = useState<AnalyzerRow[]>([]);
  const [filter, setFilter] = useState("");
  const [selected, setSelected] = useState<AnalyzerRow | null>(null);
  const [summary, setSummary] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const showError = useCallback((message: string) => setError(message), []);

  const refreshStatus = useCallback(async (api: CanLabApi) => {
    const reply = await api.request({ type: "GetStatus" });
    if (reply.type === "Status") {
      setState(reply.state);
      setLoadedPath(reply.projectPath);
      cursorRef.current = reply.nextSeq;
    } else if (reply.type === "Error") {
      showError(reply.message);
    }
  }, [showError]);

  const pollEvents = useCallback(async () => {
    const api = apiRef.current;
    if (!api?.connected) return;
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

  useEffect(() => {
    if (!connected) return;
    const timer = setInterval(pollEvents, 500);
    return () => clearInterval(timer);
  }, [connected, pollEvents]);

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
    } catch (e) {
      showError(e instanceof Error ? e.message : String(e));
    }
  }, [refreshStatus, showError]);

  const runCmd = useCallback(async (label: string, build: () => Parameters<CanLabApi["request"]>[0]) => {
    const api = apiRef.current;
    if (!api) {
      showError("not connected — press Connect first");
      return;
    }
    setError(null);
    setSummary(null);
    try {
      const reply = await api.request(build());
      const err = replyError(reply);
      if (err) {
        showError(`${label}: ${err}`);
        return;
      }
      if (reply.type === "RunSummary") {
        setSummary(`${reply.transmitted} transmitted, ${reply.received} received`);
      }
      await refreshStatus(api);
      await pollEvents();
    } catch (e) {
      showError(e instanceof Error ? e.message : String(e));
    }
  }, [pollEvents, refreshStatus, showError]);

  const load = useCallback(async () => {
    const api = apiRef.current;
    if (!api) {
      showError("not connected — press Connect first");
      return;
    }
    setError(null);
    try {
      const reply = await api.request({ type: "Load", path: projectPath });
      const err = replyError(reply);
      if (err) {
        showError(`Load: ${err}`);
        return;
      }
      const proj = await api.request({ type: "GetProject" });
      if (proj.type === "Project") {
        const p = proj.project as { buses: ProjectBus[]; nodes: ProjectNode[] };
        setBuses(p.buses ?? []);
        setNodes(p.nodes ?? []);
      }
      setRows([]);
      setSelected(null);
      cursorRef.current = 0;
      await refreshStatus(api);
    } catch (e) {
      showError(e instanceof Error ? e.message : String(e));
    }
  }, [projectPath, refreshStatus, showError]);

  const { flowNodes, edges } = useMemo(() => toFlowNodes(buses, nodes), [buses, nodes]);
  const visibleRows = useMemo(() => {
    const f = filter.trim().toLowerCase();
    if (!f) return rows;
    return rows.filter((r) => r.id.toLowerCase().includes(f) || r.node.toLowerCase().includes(f));
  }, [rows, filter]);

  const exportCsv = useCallback(() => {
    const csv = ["seq,time_ms,dir,node,id,dlc,data", ...rows.map((r) => [r.seq, (r.timeNs / 1e6).toFixed(3), r.dir, r.node, r.id, r.dlc, `"${r.data}"`].join(","))].join("\n");
    const url = URL.createObjectURL(new Blob([csv], { type: "text/csv" }));
    const a = document.createElement("a");
    a.href = url;
    a.download = "canlab-trace.csv";
    a.click();
    URL.revokeObjectURL(url);
  }, [rows]);

  const btn: React.CSSProperties = { padding: "6px 12px", borderRadius: 6, border: "1px solid #374151", background: "#1f2937", color: "#e5e7eb", cursor: "pointer" };

  return (
    <main style={{ fontFamily: "system-ui, sans-serif", background: "#030712", color: "#e5e7eb", minHeight: "100vh", padding: 16 }}>
      <header style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap", marginBottom: 12 }}>
        <h1 style={{ margin: 0, fontSize: 20 }}>CanLab</h1>
        <span aria-label={`simulation ${state}`} style={{ padding: "2px 10px", borderRadius: 999, background: state === "running" ? "#14532d" : "#1f2937", border: "1px solid #374151" }}>
          ● {state}
        </span>
        {!connected ? (
          <button style={btn} onClick={connect}>Connect</button>
        ) : (
          <>
            <input aria-label="project file path" style={{ ...btn, minWidth: 320, cursor: "text" }} value={projectPath} onChange={(e) => setProjectPath(e.target.value)} />
            <button style={btn} onClick={load}>Load</button>
            <button style={btn} onClick={() => runCmd("Start", () => ({ type: "Start" }))}>▶ Run</button>
            <button style={btn} onClick={() => runCmd("Pause", () => ({ type: "Pause" }))}>⏸ Pause</button>
            <button style={btn} onClick={() => runCmd("Resume", () => ({ type: "Resume" }))}>Resume</button>
            <button style={btn} onClick={() => runCmd("Stop", () => ({ type: "Stop" }))}>⏹ Stop</button>
            <button style={btn} onClick={() => runCmd("Reset", () => ({ type: "Reset" }))}>↻ Reset</button>
            <button style={btn} onClick={() => runCmd("Step", () => ({ type: "Step", deltaNs: 238_000 }))}>⏭ Step</button>
          </>
        )}
      </header>

      {error && (
        <div role="alert" style={{ background: "#7f1d1d", border: "1px solid #ef4444", borderRadius: 8, padding: 10, marginBottom: 12, whiteSpace: "pre-wrap" }}>
          {error}
        </div>
      )}
      {summary && <p style={{ color: "#86efac" }}>Run finished: {summary}.</p>}
      {loadedPath && <p style={{ color: "#9ca3af" }}>Project: {loadedPath} · run <code>canlab serve</code> from the repo root so relative paths resolve.</p>}

      <section aria-label="network canvas" style={{ height: 380, background: "#0b1220", borderRadius: 12, border: "1px solid #1f2937", marginBottom: 12 }}>
        {flowNodes.length === 0 ? (
          <p style={{ padding: 16, color: "#9ca3af" }}>Connect, then Load a project to see its network topology. Editing (drag-and-drop, save) lands in the next slice.</p>
        ) : (
          <ReactFlow nodes={flowNodes} edges={edges} fitView>
            <Background />
            <Controls />
          </ReactFlow>
        )}
      </section>

      <section aria-label="can analyzer" style={{ background: "#0b1220", borderRadius: 12, border: "1px solid #1f2937", padding: 12 }}>
        <div style={{ display: "flex", gap: 8, alignItems: "center", marginBottom: 8 }}>
          <h2 style={{ margin: 0, fontSize: 16 }}>CAN Analyzer</h2>
          <input aria-label="filter by id or node" placeholder="filter: id or node" style={{ ...btn, cursor: "text" }} value={filter} onChange={(e) => setFilter(e.target.value)} />
          <button style={btn} onClick={exportCsv}>Export CSV</button>
          <span style={{ color: "#9ca3af" }}>{visibleRows.length} row(s){rows.length >= MAX_ROWS ? ` (capped at ${MAX_ROWS})` : ""}</span>
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
                <tr key={r.seq} onClick={() => setSelected(r)} style={{ cursor: "pointer", background: selected?.seq === r.seq ? "#1e3a8a" : "transparent" }}>
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
              <dl style={{ margin: 0, fontSize: 13 }}>
                <dt>ID</dt><dd style={{ fontFamily: "monospace" }}>{selected.id}</dd>
                <dt>Direction</dt><dd>{selected.dir}</dd>
                <dt>Node</dt><dd>{selected.node}</dd>
                <dt>DLC</dt><dd>{selected.dlc}</dd>
                <dt>Data</dt><dd style={{ fontFamily: "monospace" }}>{selected.data || "(empty)"}</dd>
                <dt>Timestamp</dt><dd>{fmtTimeNs(selected.timeNs)}</dd>
              </dl>
            ) : (
              <p style={{ color: "#9ca3af" }}>Click a frame to inspect it.</p>
            )}
          </aside>
        </div>
      </section>
    </main>
  );
}
