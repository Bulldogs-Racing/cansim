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
  fmtTimeNs,
  KNOWN_DEVICES,
  MessageDecl,
  NodeDecl,
  ProjectBus,
  ProjectNode,
  SeqEvent,
  ServerMsg,
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
  const positionsRef = useRef(new Map<string, { x: number; y: number }>());
  const [connected, setConnected] = useState(false);
  const [state, setState] = useState<EngineState>("idle");
  const [dirty, setDirty] = useState(false);
  const [projectPath, setProjectPath] = useState("examples/two_nodes.canlab.yaml");
  const [loadedPath, setLoadedPath] = useState<string | null>(null);
  const [buses, setBuses] = useState<ProjectBus[]>([]);
  const [nodes, setNodes] = useState<ProjectNode[]>([]);
  const [messages, setMessages] = useState<MessageDecl[]>([]);
  const [flowNodes, setFlowNodes] = useState<Node[]>([]);
  const [flowEdges, setFlowEdges] = useState<Edge[]>([]);
  const [canvasSel, setCanvasSel] = useState<string | null>(null);
  const [rows, setRows] = useState<AnalyzerRow[]>([]);
  const [filter, setFilter] = useState("");
  const [selected, setSelected] = useState<AnalyzerRow | null>(null);
  const [summary, setSummary] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const showError = useCallback((message: string) => setError(message), []);

  /** Adopt a Status reply: engine state, dirty chip, and event cursor. Rows
   *  past the cursor belong to a previous engine incarnation — drop them so
   *  the analyzer always reflects the current log. */
  const applyStatus = useCallback((reply: StatusMsg) => {
    setState(reply.state);
    setLoadedPath(reply.projectPath);
    setDirty(reply.dirty);
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
      await refreshProject(api); // picks up a --project preload, if any
    } catch (e) {
      showError(e instanceof Error ? e.message : String(e));
    }
  }, [refreshStatus, refreshProject, showError]);

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
      }
      await refreshStatus(api);
      await pollEvents();
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
      if (reply.type === "Status") applyStatus(reply);
      await refreshProject(api);
    } catch (e) {
      showError(e instanceof Error ? e.message : String(e));
    }
  }, [applyStatus, refreshProject, showError]);

  const load = useCallback(() => editCmd("Load", { type: "Load", path: projectPath }), [editCmd, projectPath]);

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
          </>
        )}
      </header>

      {error && (
        <div role="alert" style={{ background: "#7f1d1d", border: "1px solid #ef4444", borderRadius: 8, padding: 10, marginBottom: 12, whiteSpace: "pre-wrap" }}>
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
              onApply={(next) => editCmd("Update", { type: "UpdateNode", id: selNode.id, node: next })}
              onDelete={() => removeFlowNode(`node:${selNode.id}`)}
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
          key={nodes.map((n) => n.id).join(",")}
          messages={messages}
          nodes={nodes}
          onAdd={(message) => editCmd("AddMessage", { type: "AddMessage", message })}
          onRemove={(index) => editCmd("RemoveMessage", { type: "RemoveMessage", index })}
        />
      )}

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

/** Scripted traffic editor: what Run transmits, in order. Add form parses
 *  hex (`0x123` / `01 02`) and reports parse problems inline, before the
 *  server ever sees them; server-side validation is the backstop. */
function MessagesPanel({ messages, nodes, onAdd, onRemove }: {
  messages: MessageDecl[];
  nodes: ProjectNode[];
  onAdd: (message: MessageDecl) => void;
  onRemove: (index: number) => void;
}): JSX.Element {
  const [sender, setSender] = useState(nodes[0]?.id ?? "");
  const [idText, setIdText] = useState("0x123");
  const [dataText, setDataText] = useState("01 02 03 04");
  const [extended, setExtended] = useState(false);
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
    onAdd({ sender, id, data, extended });
  };

  return (
    <section aria-label="scripted messages" style={{ background: "#0b1220", borderRadius: 12, border: "1px solid #1f2937", padding: 12, marginBottom: 12 }}>
      <h2 style={{ margin: "0 0 8px", fontSize: 16 }}>Scripted traffic ({messages.length})</h2>
      <p style={{ margin: "0 0 8px", color: "#9ca3af", fontSize: 13 }}>
        Frames Run transmits in order. Deleting a row shifts later indices (see bugs.md); the list refreshes after every op.
      </p>
      {messages.length > 0 && (
        <table style={{ width: "100%", borderCollapse: "collapse", fontSize: 13, marginBottom: 8 }}>
          <thead>
            <tr style={{ textAlign: "left", color: "#9ca3af" }}>
              <th>#</th><th>Sender</th><th>ID</th><th>DLC</th><th>Data</th><th></th>
            </tr>
          </thead>
          <tbody>
            {messages.map((m, i) => (
              <tr key={i}>
                <td>{i}</td>
                <td>{m.sender}</td>
                <td style={{ fontFamily: "monospace" }}>0x{m.id.toString(16).toUpperCase()}{m.extended ? " (ext)" : ""}</td>
                <td>{m.data.length}</td>
                <td style={{ fontFamily: "monospace" }}>{m.data.map((b) => b.toString(16).toUpperCase().padStart(2, "0")).join(" ") || "(empty)"}</td>
                <td><button style={btn} onClick={() => onRemove(i)}>Delete</button></td>
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
        <button style={btn} onClick={submit}>＋ Add frame</button>
      </div>
      {formError && <p role="alert" style={{ color: "#fca5a5", margin: "8px 0 0" }}>{formError}</p>}
    </section>
  );
}

function NodeProps({ node, buses, onApply, onDelete }: {
  node: ProjectNode;
  buses: ProjectBus[];
  onApply: (next: NodeDecl) => void;
  onDelete: () => void;
}): JSX.Element {
  const [device, setDevice] = useState(node.device);
  const [firmware, setFirmware] = useState(node.firmware ?? "");
  const [bus, setBus] = useState(node.can.bus);
  const field: React.CSSProperties = { display: "block", width: "100%", boxSizing: "border-box", margin: "4px 0 10px", padding: 6, borderRadius: 6, border: "1px solid #374151", background: "#111827", color: "#e5e7eb" };
  const btn: React.CSSProperties = { padding: "6px 12px", borderRadius: 6, border: "1px solid #374151", background: "#1f2937", color: "#e5e7eb", cursor: "pointer", marginRight: 8 };
  return (
    <div>
      <p style={{ margin: "0 0 8px" }}><strong>{node.id}</strong> · backend virtual (live session runs virtual nodes only)</p>
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
